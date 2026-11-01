//! Keeping one follow task per active stream.
//!
//! The database says what should run, so a restart resumes every stream (FR-S5), and a `stop`
//! from any process reaches its task at the next look.

use crate::runner::now_ms;
use dalsegno_app::fetch::{SharedDatabase, lock};
use dalsegno_app::follow::FollowEnd;
use dalsegno_app::notify::STREAM;
use dalsegno_store::database::{Database, StoreError};
use dalsegno_store::streams::{StreamRow, StreamState};
use serde_json::json;
use std::collections::HashMap;
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep};

/// How often the database is looked at.
const POLL_INTERVAL: Duration = Duration::from_secs(5);
/// The first wait after a failure. It doubles up to the limit, so an outage does not stop a
/// stream for good and does not turn into a stream of yt-dlp calls either.
const RETRY_BASE: Duration = Duration::from_secs(30);
const RETRY_LIMIT: Duration = Duration::from_secs(600);
/// A refusal of every address waits far longer, as each try is another request from an address
/// the origin is already suspicious of. This matches how long a refused family is avoided.
const REFUSED_BASE: Duration = Duration::from_secs(3600);
const REFUSED_LIMIT: Duration = Duration::from_secs(6 * 3600);

const FOLLOWING_FAILED: &str = "following failed, and is being retried";
const ORIGIN_REFUSED: &str =
    "the origin refused every address, which can mean a bot check, and is tried again in hours";

const ACTIVE: [StreamState; 2] = [StreamState::Backfilling, StreamState::Recording];

/// What following a stream gives back, with a message safe to store and show.
pub type FollowFuture = Pin<Box<dyn Future<Output = Result<FollowEnd, String>> + Send>>;

/// Follows one stream until it is told to stop.
pub trait Follower: Debug + Send + Sync + 'static {
    /// Starts following.
    fn follow(&self, row: StreamRow, stop: watch::Receiver<bool>) -> FollowFuture;
}

#[derive(Debug)]
struct Running {
    stop: watch::Sender<bool>,
    task: JoinHandle<Result<FollowEnd, String>>,
}

#[derive(Debug, Clone, Copy)]
struct Retry {
    failures: u32,
    at: Instant,
}

/// Runs the follow tasks.
#[derive(Debug)]
pub struct Supervisor {
    database: SharedDatabase,
    follower: Arc<dyn Follower>,
    poll: Duration,
    retry_base: Duration,
    refused_base: Duration,
    running: HashMap<i64, Running>,
    retries: HashMap<i64, Retry>,
}

fn delay_after(base: Duration, limit: Duration, failures: u32) -> Duration {
    let doubled = base.saturating_mul(1 << failures.saturating_sub(1).min(16));
    doubled.min(limit)
}

/// Records how a task ended, unless a person stopped the stream meanwhile.
fn record_end(
    database: &Database,
    id: i64,
    state: StreamState,
    reason: &str,
) -> Result<bool, StoreError> {
    let is_active = database
        .stream_state(id)?
        .is_some_and(|(current, _)| ACTIVE.contains(&current));
    if is_active {
        database.set_stream_state(id, state, Some(reason))?;
    }
    Ok(is_active)
}

/// Records how a task ended, with its notification. An outage alerts once, not on every retry.
fn record_with_notice(
    database: &Database,
    id: i64,
    state: StreamState,
    words: &str,
    reason: &str,
    subject: &str,
    should_notify: bool,
) -> Result<(), StoreError> {
    database.atomically(|database| {
        let changed = record_end(database, id, state, reason)?;
        if changed && should_notify {
            let title = database.stream(id)?.map(|row| row.stream.title);
            let body = json!({
                "stream": title,
                "message": format!("{words}: {reason}"),
            });
            database.enqueue_notification(STREAM, subject, &body.to_string(), now_ms())?;
        }
        Ok(())
    })
}

impl Supervisor {
    /// Makes a supervisor.
    pub fn new(database: SharedDatabase, follower: Arc<dyn Follower>) -> Self {
        Self {
            database,
            follower,
            poll: POLL_INTERVAL,
            retry_base: RETRY_BASE,
            refused_base: REFUSED_BASE,
            running: HashMap::new(),
            retries: HashMap::new(),
        }
    }

    /// Runs until `shutdown` turns true, then stops every task.
    pub async fn run(mut self, mut shutdown: watch::Receiver<bool>) {
        while !*shutdown.borrow() {
            if let Err(error) = self.look().await {
                tracing::error!(error, "cannot look at the streams");
            }
            tokio::select! {
                () = sleep(self.poll) => {}
                _ = shutdown.changed() => {}
            }
        }
        for (_, running) in self.running.drain() {
            running.stop.send_replace(true);
            let _ = running.task.await;
        }
    }

    async fn look(&mut self) -> Result<(), String> {
        self.reap().await?;
        self.retry_failed()?;
        let active = {
            let database = lock(&self.database).map_err(|error| error.to_string())?;
            database
                .streams_in_states(&ACTIVE)
                .map_err(|error| error.to_string())?
        };
        for (id, running) in &self.running {
            if !active.iter().any(|row| row.id == *id) {
                running.stop.send_replace(true);
            }
        }
        for row in active {
            self.running
                .entry(row.id)
                .or_insert_with(|| spawn(&self.follower, row));
        }
        Ok(())
    }

    async fn reap(&mut self) -> Result<(), String> {
        let finished: Vec<i64> = self
            .running
            .iter()
            .filter(|(_, running)| running.task.is_finished())
            .map(|(id, _)| *id)
            .collect();
        for id in finished {
            let Some(running) = self.running.remove(&id) else {
                continue;
            };
            let outcome = running
                .task
                .await
                .unwrap_or_else(|_| Err("following stopped unexpectedly".to_string()));
            self.record(id, outcome)?;
        }
        Ok(())
    }

    fn record(&mut self, id: i64, outcome: Result<FollowEnd, String>) -> Result<(), String> {
        match outcome {
            Ok(FollowEnd::Stopped) => Ok(()),
            Ok(FollowEnd::Ended(reason)) => {
                let database = lock(&self.database).map_err(|error| error.to_string())?;
                let subject = format!("stream:{id}:ended");
                let words = "the stream has ended";
                record_with_notice(
                    &database,
                    id,
                    StreamState::Ended,
                    words,
                    &reason,
                    &subject,
                    true,
                )
                .map_err(|error| error.to_string())
            }
            Ok(FollowEnd::Refused(reason)) => {
                let wait = (self.refused_base, REFUSED_LIMIT.max(self.refused_base));
                self.record_failure(id, &reason, ORIGIN_REFUSED, wait)
            }
            Err(reason) => {
                let wait = (self.retry_base, RETRY_LIMIT.max(self.retry_base));
                self.record_failure(id, &reason, FOLLOWING_FAILED, wait)
            }
        }
    }

    /// Records a failure and when to try again. An outage alerts once, not on every retry.
    fn record_failure(
        &mut self,
        id: i64,
        reason: &str,
        words: &str,
        (base, limit): (Duration, Duration),
    ) -> Result<(), String> {
        tracing::warn!(id, reason, "following failed");
        let failures = self.retries.get(&id).map_or(0, |retry| retry.failures) + 1;
        let at = Instant::now() + delay_after(base, limit, failures);
        self.retries.insert(id, Retry { failures, at });
        let database = lock(&self.database).map_err(|error| error.to_string())?;
        let subject = format!("stream:{id}:error:{}", now_ms());
        let is_first = failures == 1;
        record_with_notice(
            &database,
            id,
            StreamState::Error,
            words,
            reason,
            &subject,
            is_first,
        )
        .map_err(|error| error.to_string())
    }

    /// Gives failed streams another go when their wait is over. A stream that failed before a
    /// restart has no wait left.
    fn retry_failed(&mut self) -> Result<(), String> {
        let database = lock(&self.database).map_err(|error| error.to_string())?;
        let rows = database
            .streams_in_states(&[StreamState::Error])
            .map_err(|error| error.to_string())?;
        for row in rows {
            let is_due = self
                .retries
                .get(&row.id)
                .is_none_or(|retry| retry.at <= Instant::now());
            if is_due {
                database
                    .set_stream_state(row.id, StreamState::Backfilling, None)
                    .map_err(|error| error.to_string())?;
            }
        }
        let recording = database
            .streams_in_states(&[StreamState::Recording])
            .map_err(|error| error.to_string())?;
        for row in recording {
            self.retries.remove(&row.id);
        }
        Ok(())
    }
}

fn spawn(follower: &Arc<dyn Follower>, row: StreamRow) -> Running {
    let (stop, stopped) = watch::channel(false);
    Running {
        stop,
        task: tokio::spawn(follower.follow(row, stopped)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dalsegno_store::streams::NewStream;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::timeout;

    const WAIT: Duration = Duration::from_secs(5);

    /// Ends the way it is told to, or runs until stopped.
    #[derive(Debug, Default)]
    struct Scripted {
        ends: Mutex<Vec<Result<FollowEnd, String>>>,
        started: AtomicUsize,
        stopped: Arc<AtomicUsize>,
    }

    impl Follower for Arc<Scripted> {
        fn follow(&self, _row: StreamRow, mut stop: watch::Receiver<bool>) -> FollowFuture {
            self.started.fetch_add(1, Ordering::SeqCst);
            let end = self.ends.lock().unwrap().pop();
            let stopped = Arc::clone(&self.stopped);
            Box::pin(async move {
                if let Some(end) = end {
                    return end;
                }
                let _ = stop.wait_for(|stop| *stop).await;
                stopped.fetch_add(1, Ordering::SeqCst);
                Ok(FollowEnd::Stopped)
            })
        }
    }

    fn database_with_stream(state: StreamState) -> (SharedDatabase, i64) {
        let database = Database::open_in_memory().unwrap();
        let id = database
            .insert_stream(&NewStream {
                video_id: "abcDEF123_-".to_string(),
                title: "Example TV".to_string(),
                channel_id: None,
                channel_name: None,
                thumbnail_url: None,
                video_itag: None,
                audio_itag: 140,
                fragment_len: 5.0,
                registered_at: 0,
            })
            .unwrap();
        database.set_stream_state(id, state, None).unwrap();
        (Arc::new(Mutex::new(database)), id)
    }

    fn supervisor(database: &SharedDatabase, follower: &Arc<Scripted>) -> Supervisor {
        Supervisor {
            database: Arc::clone(database),
            follower: Arc::new(Arc::clone(follower)),
            poll: Duration::from_millis(5),
            retry_base: Duration::from_millis(5),
            refused_base: Duration::from_millis(5),
            running: HashMap::new(),
            retries: HashMap::new(),
        }
    }

    fn state(database: &SharedDatabase, id: i64) -> (StreamState, Option<String>) {
        lock(database).unwrap().stream_state(id).unwrap().unwrap()
    }

    async fn until(what: impl Fn() -> bool) {
        timeout(WAIT, async {
            while !what() {
                sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn an_active_stream_is_followed_once() {
        let (database, _) = database_with_stream(StreamState::Recording);
        let follower = Arc::new(Scripted::default());
        let (stop, shutdown) = watch::channel(false);
        let running = tokio::spawn(supervisor(&database, &follower).run(shutdown));

        until(|| follower.started.load(Ordering::SeqCst) > 0).await;
        sleep(Duration::from_millis(30)).await;
        stop.send(true).unwrap();
        running.await.unwrap();

        assert_eq!(follower.started.load(Ordering::SeqCst), 1);
        assert_eq!(follower.stopped.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_stream_not_started_is_left_alone() {
        let (database, _) = database_with_stream(StreamState::Registered);
        let follower = Arc::new(Scripted::default());
        let (stop, shutdown) = watch::channel(false);
        let running = tokio::spawn(supervisor(&database, &follower).run(shutdown));

        sleep(Duration::from_millis(30)).await;
        stop.send(true).unwrap();
        running.await.unwrap();

        assert_eq!(follower.started.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn pausing_stops_the_task() {
        let (database, id) = database_with_stream(StreamState::Recording);
        let follower = Arc::new(Scripted::default());
        let (stop, shutdown) = watch::channel(false);
        let running = tokio::spawn(supervisor(&database, &follower).run(shutdown));
        until(|| follower.started.load(Ordering::SeqCst) > 0).await;

        lock(&database)
            .unwrap()
            .set_stream_state(id, StreamState::Paused, None)
            .unwrap();

        until(|| follower.stopped.load(Ordering::SeqCst) > 0).await;
        stop.send(true).unwrap();
        running.await.unwrap();
        assert_eq!(state(&database, id).0, StreamState::Paused);
    }

    #[tokio::test]
    async fn a_finished_stream_ends_with_its_reason() {
        let (database, id) = database_with_stream(StreamState::Recording);
        let follower = Arc::new(Scripted::default());
        follower
            .ends
            .lock()
            .unwrap()
            .push(Ok(FollowEnd::Ended("has ended".to_string())));
        let (stop, shutdown) = watch::channel(false);
        let running = tokio::spawn(supervisor(&database, &follower).run(shutdown));

        until(|| state(&database, id).0 == StreamState::Ended).await;
        stop.send(true).unwrap();
        running.await.unwrap();

        assert_eq!(state(&database, id).1.as_deref(), Some("has ended"));
    }

    #[tokio::test]
    async fn a_failed_stream_shows_the_reason_and_is_tried_again() {
        let (database, id) = database_with_stream(StreamState::Recording);
        let follower = Arc::new(Scripted::default());
        follower
            .ends
            .lock()
            .unwrap()
            .push(Err("offline".to_string()));
        let (stop, shutdown) = watch::channel(false);
        let mut watched = supervisor(&database, &follower);
        watched.retry_base = Duration::from_millis(40);
        let running = tokio::spawn(watched.run(shutdown));

        until(|| state(&database, id).0 == StreamState::Error).await;
        assert_eq!(state(&database, id).1.as_deref(), Some("offline"));
        until(|| follower.started.load(Ordering::SeqCst) == 2).await;
        stop.send(true).unwrap();
        running.await.unwrap();

        assert_eq!(state(&database, id), (StreamState::Backfilling, None));
    }

    #[tokio::test]
    async fn a_stream_that_failed_before_a_restart_is_tried_at_once() {
        let (database, _) = database_with_stream(StreamState::Error);
        let follower = Arc::new(Scripted::default());
        let (stop, shutdown) = watch::channel(false);
        let running = tokio::spawn(supervisor(&database, &follower).run(shutdown));

        until(|| follower.started.load(Ordering::SeqCst) == 1).await;
        stop.send(true).unwrap();
        running.await.unwrap();
    }

    fn notices(database: &SharedDatabase) -> usize {
        lock(database)
            .unwrap()
            .pending_notifications(10)
            .unwrap()
            .len()
    }

    #[tokio::test]
    async fn a_failing_stream_alerts_once_however_often_it_retries() {
        let (database, id) = database_with_stream(StreamState::Recording);
        let follower = Arc::new(Scripted::default());
        follower.ends.lock().unwrap().extend([
            Err("offline".to_string()),
            Err("offline".to_string()),
            Err("offline".to_string()),
        ]);
        let (stop, shutdown) = watch::channel(false);
        let running = tokio::spawn(supervisor(&database, &follower).run(shutdown));

        until(|| follower.started.load(Ordering::SeqCst) >= 4).await;
        stop.send(true).unwrap();
        running.await.unwrap();

        assert_eq!(notices(&database), 1);
        assert_eq!(state(&database, id).0, StreamState::Backfilling);
    }

    #[tokio::test]
    async fn a_finished_stream_is_announced() {
        let (database, id) = database_with_stream(StreamState::Recording);
        let follower = Arc::new(Scripted::default());
        follower
            .ends
            .lock()
            .unwrap()
            .push(Ok(FollowEnd::Ended("has ended".to_string())));
        let (stop, shutdown) = watch::channel(false);
        let running = tokio::spawn(supervisor(&database, &follower).run(shutdown));

        until(|| state(&database, id).0 == StreamState::Ended).await;
        stop.send(true).unwrap();
        running.await.unwrap();

        assert_eq!(notices(&database), 1);
    }

    #[tokio::test]
    async fn a_refused_stream_waits_far_longer_and_says_why() {
        let (database, id) = database_with_stream(StreamState::Recording);
        let follower = Arc::new(Scripted::default());
        follower
            .ends
            .lock()
            .unwrap()
            .push(Ok(FollowEnd::Refused("no usable IP family".to_string())));
        let (stop, shutdown) = watch::channel(false);
        let mut watched = supervisor(&database, &follower);
        watched.refused_base = Duration::from_secs(3600);
        let running = tokio::spawn(watched.run(shutdown));

        until(|| state(&database, id).0 == StreamState::Error).await;
        sleep(Duration::from_millis(40)).await;
        stop.send(true).unwrap();
        running.await.unwrap();

        assert_eq!(follower.started.load(Ordering::SeqCst), 1);
        assert_eq!(state(&database, id).0, StreamState::Error);
        let body = lock(&database).unwrap().pending_notifications(1).unwrap()[0]
            .body
            .clone();
        assert!(body.contains("bot check"), "{body}");
    }

    #[tokio::test]
    async fn a_failure_after_a_pause_keeps_the_pause() {
        let (database, id) = database_with_stream(StreamState::Paused);

        record_end(&lock(&database).unwrap(), id, StreamState::Error, "offline").unwrap();

        assert_eq!(state(&database, id).0, StreamState::Paused);
    }

    #[test]
    fn waits_double_up_to_the_limit() {
        assert_eq!(delay_after(RETRY_BASE, RETRY_LIMIT, 1), RETRY_BASE);
        assert_eq!(delay_after(RETRY_BASE, RETRY_LIMIT, 2), RETRY_BASE * 2);
        assert_eq!(delay_after(RETRY_BASE, RETRY_LIMIT, 500), RETRY_LIMIT);
        assert_eq!(delay_after(REFUSED_BASE, REFUSED_LIMIT, 500), REFUSED_LIMIT);
    }
}

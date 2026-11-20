//! Live updates for the Web UI over SSE (FR-J2, D-33).
//!
//! A watcher reads the jobs, streams and changes the API lists, every second, and publishes
//! what differs from its last reading, so no writer has to announce anything. Each event
//! carries the same object the list endpoint returns, so a screen replaces it in place.
//!
//! One task publishes, so subscribers see events in order. Subscribers read from a bounded
//! channel: a slow one falls behind and is told to refetch, and never holds the service up.

use crate::api::{change_json, job_json, stream_json};
use axum::response::sse;
use dalsegno_app::fetch::{SharedDatabase, lock};
use dalsegno_store::database::Database;
use futures_util::{Stream, StreamExt, stream};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, watch};
use tokio::task::spawn_blocking;
use tokio::time::sleep;

/// How often the watcher reads the database.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How many events a subscriber may fall behind before it is told to refetch.
pub const CAPACITY: usize = 256;
/// The newest jobs and changes the watcher reads. Jobs come a few an hour for each stream and changes fewer,
/// so whatever a screen is watching move is among them.
const WINDOW: u32 = 200;
/// Every stream is read; there are a handful.
const STREAMS: u32 = 1000;
/// Opens every subscription and goes to one that fell behind, which then refetches what it
/// shows. It carries the version of the running service.
const RESYNC: &str = "resync";

/// What an event is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    Job,
    Stream,
    Change,
}

impl Topic {
    /// The SSE event name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Job => "job.progress",
            Self::Stream => "stream.state",
            Self::Change => "change.updated",
        }
    }
}

/// One update, numbered in the order it was published.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub seq: u64,
    pub topic: Topic,
    pub data: Value,
}

/// What the watcher read once: each listed object by its ID.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub jobs: BTreeMap<i64, Value>,
    pub streams: BTreeMap<i64, Value>,
    pub changes: BTreeMap<i64, Value>,
}

fn by_id(items: impl Iterator<Item = Value>) -> BTreeMap<i64, Value> {
    items
        .filter_map(|item| item["id"].as_i64().map(|id| (id, item)))
        .collect()
}

impl Snapshot {
    /// Reads what the list endpoints show.
    pub fn read(database: &Database) -> Result<Self, String> {
        let jobs = database.list_jobs(WINDOW, 0).map_err(|e| e.to_string())?;
        let streams = database
            .list_streams(STREAMS, 0)
            .map_err(|e| e.to_string())?;
        let changes = database
            .list_changes(WINDOW, 0)
            .map_err(|e| e.to_string())?;
        Ok(Self {
            jobs: by_id(jobs.iter().map(job_json)),
            streams: by_id(streams.iter().map(stream_json)),
            changes: by_id(changes.iter().map(change_json)),
        })
    }
}

fn changed<'a>(
    topic: Topic,
    before: &'a BTreeMap<i64, Value>,
    after: &'a BTreeMap<i64, Value>,
) -> impl Iterator<Item = (Topic, Value)> + 'a {
    after
        .iter()
        .filter(move |(id, item)| before.get(id) != Some(item))
        .map(move |(_, item)| (topic, item.clone()))
}

/// What is new or different in `after`, jobs first, then streams, then changes, each by ID.
/// Something that left the window is not an update.
pub fn diff(before: &Snapshot, after: &Snapshot) -> Vec<(Topic, Value)> {
    changed(Topic::Job, &before.jobs, &after.jobs)
        .chain(changed(Topic::Stream, &before.streams, &after.streams))
        .chain(changed(Topic::Change, &before.changes, &after.changes))
        .collect()
}

/// Hands events to every subscriber.
#[derive(Debug, Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Arc<Event>>,
    seq: Arc<AtomicU64>,
    closed: Arc<watch::Sender<bool>>,
}

impl EventBus {
    /// Makes a bus whose subscribers may fall `capacity` events behind.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        let (closed, _) = watch::channel(false);
        Self {
            sender,
            seq: Arc::new(AtomicU64::new(0)),
            closed: Arc::new(closed),
        }
    }

    /// Publishes an update. It never waits, and with nobody listening it is dropped.
    pub fn publish(&self, topic: Topic, data: Value) {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = self.sender.send(Arc::new(Event { seq, topic, data }));
    }

    /// Ends every subscription, as the service stops.
    pub fn close(&self) {
        self.closed.send_replace(true);
    }

    /// A resync, then the events from now on as SSE, until the bus closes.
    pub fn subscribe(&self) -> impl Stream<Item = Result<sse::Event, Infallible>> + Send + use<> {
        let mut closed = self.closed.subscribe();
        let updates = stream::unfold(self.sender.subscribe(), |mut receiver| async move {
            let message = match receiver.recv().await {
                Ok(event) => to_sse(&event),
                Err(RecvError::Lagged(_)) => resync(),
                Err(RecvError::Closed) => return None,
            };
            Some((Ok(message), receiver))
        });
        stream::once(async { Ok(resync()) })
            .chain(updates)
            .take_until(async move {
                let _ = closed.wait_for(|is_closed| *is_closed).await;
            })
    }
}

fn resync() -> sse::Event {
    let data = json!({ "version": env!("CARGO_PKG_VERSION") });
    sse::Event::default().event(RESYNC).data(data.to_string())
}

fn to_sse(event: &Event) -> sse::Event {
    sse::Event::default()
        .id(event.seq.to_string())
        .event(event.topic.name())
        .data(event.data.to_string())
}

async fn read_snapshot(database: &SharedDatabase) -> Result<Snapshot, String> {
    let database = database.clone();
    spawn_blocking(move || {
        let guard = lock(&database).map_err(|error| error.to_string())?;
        Snapshot::read(&guard)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Publishes what changes in the database until the service stops, then closes the bus. It
/// reads even with nobody subscribed, so a page that subscribes and then fetches misses
/// nothing in between.
pub async fn run_watcher(
    database: SharedDatabase,
    bus: EventBus,
    poll: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut last: Option<Snapshot> = None;
    loop {
        tokio::select! {
            () = sleep(poll) => {}
            _ = shutdown.wait_for(|is_stopping| *is_stopping) => break,
        }
        match read_snapshot(&database).await {
            Ok(now) => {
                if let Some(before) = &last {
                    diff(before, &now)
                        .into_iter()
                        .for_each(|(topic, data)| bus.publish(topic, data));
                }
                last = Some(now);
            }
            Err(error) => tracing::warn!(error, "live updates could not read the database"),
        }
    }
    bus.close();
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Bytes;
    use dalsegno_app::acquisition::Library;
    use dalsegno_store::jobs::{JobClass, NewJob};
    use std::pin::pin;
    use tempfile::tempdir;
    use tokio::time::timeout;

    const WAIT: Duration = Duration::from_secs(5);

    fn snapshot(jobs: &[Value]) -> Snapshot {
        Snapshot {
            jobs: by_id(jobs.iter().cloned()),
            ..Snapshot::default()
        }
    }

    /// Renders events as their wire text, which is what a browser reads.
    async fn wire(
        events: impl Stream<Item = Result<sse::Event, Infallible>> + Send + 'static,
        count: usize,
    ) -> String {
        let body = axum::response::Sse::new(events.take(count));
        let response = axum::response::IntoResponse::into_response(body);
        let bytes: Bytes = timeout(WAIT, axum::body::to_bytes(response.into_body(), 1 << 20))
            .await
            .unwrap()
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    /// What a subscription opens with, and a lagging one is sent.
    fn resync_wire() -> String {
        format!(
            "event: resync\ndata: {{\"version\":\"{}\"}}\n\n",
            env!("CARGO_PKG_VERSION")
        )
    }

    #[test]
    fn a_new_or_different_object_is_an_update() {
        let before = snapshot(&[json!({"id": 1, "progress": 0.1}), json!({"id": 2})]);
        let after = snapshot(&[
            json!({"id": 1, "progress": 0.5}),
            json!({"id": 2}),
            json!({"id": 3}),
        ]);

        let updates = diff(&before, &after);

        assert_eq!(
            updates,
            vec![
                (Topic::Job, json!({"id": 1, "progress": 0.5})),
                (Topic::Job, json!({"id": 3})),
            ]
        );
    }

    #[test]
    fn an_object_that_left_the_window_is_not_an_update() {
        let before = snapshot(&[json!({"id": 1}), json!({"id": 2})]);
        let after = snapshot(&[json!({"id": 2})]);

        assert!(diff(&before, &after).is_empty());
    }

    #[test]
    fn jobs_come_before_streams_and_changes() {
        let after = Snapshot {
            jobs: by_id([json!({"id": 9})].into_iter()),
            streams: by_id([json!({"id": 1})].into_iter()),
            changes: by_id([json!({"id": 4})].into_iter()),
        };

        let topics: Vec<Topic> = diff(&Snapshot::default(), &after)
            .into_iter()
            .map(|(topic, _)| topic)
            .collect();

        assert_eq!(topics, [Topic::Job, Topic::Stream, Topic::Change]);
    }

    #[tokio::test]
    async fn events_reach_a_subscriber_in_order() {
        let bus = EventBus::new(CAPACITY);
        let events = bus.subscribe();

        bus.publish(Topic::Job, json!({"id": 1}));
        bus.publish(Topic::Stream, json!({"id": 2}));
        bus.publish(Topic::Change, json!({"id": 3}));
        let text = wire(events, 4).await;

        assert_eq!(
            text,
            resync_wire()
                + "id: 1\nevent: job.progress\ndata: {\"id\":1}\n\n\
             id: 2\nevent: stream.state\ndata: {\"id\":2}\n\n\
             id: 3\nevent: change.updated\ndata: {\"id\":3}\n\n"
        );
    }

    #[tokio::test]
    async fn a_slow_subscriber_cannot_hold_up_publishing() {
        let bus = EventBus::new(2);
        let slow = bus.subscribe();
        let quick = bus.subscribe();

        // Nothing reads yet, and publishing past the capacity still returns at once.
        (1..=5).for_each(|id| bus.publish(Topic::Job, json!({ "id": id })));
        let quick_text = wire(quick, 2).await;
        let slow_text = wire(slow, 4).await;

        assert!(quick_text.starts_with(&resync_wire()));
        assert!(quick_text.ends_with(&resync_wire()));
        assert_eq!(
            slow_text,
            resync_wire()
                + &resync_wire()
                + "id: 4\nevent: job.progress\ndata: {\"id\":4}\n\n\
                   id: 5\nevent: job.progress\ndata: {\"id\":5}\n\n"
        );
    }

    #[tokio::test]
    async fn every_subscription_opens_with_the_running_version() {
        let bus = EventBus::new(CAPACITY);

        let text = wire(bus.subscribe(), 1).await;

        assert_eq!(text, resync_wire());
    }

    #[tokio::test]
    async fn closing_the_bus_ends_every_subscription() {
        let bus = EventBus::new(CAPACITY);
        let events = bus.subscribe();

        bus.close();
        let remaining = timeout(WAIT, events.collect::<Vec<_>>()).await.unwrap();

        assert!(remaining.is_empty());
    }

    #[tokio::test]
    async fn the_watcher_publishes_a_job_that_moves_and_closes_on_stop() {
        let root = tempdir().unwrap();
        let database = Library::open(root.path()).unwrap().database();
        let bus = EventBus::new(CAPACITY);
        let (stop, shutdown) = watch::channel(false);
        let watcher = tokio::spawn(run_watcher(
            database.clone(),
            bus.clone(),
            Duration::from_millis(10),
            shutdown,
        ));
        let mut events = pin!(bus.subscribe().skip(1));
        // Let the watcher take its first reading, which publishes nothing.
        sleep(Duration::from_millis(50)).await;

        let job = NewJob {
            kind: "sweep".to_string(),
            class: JobClass::Cpu,
            target: "{}".to_string(),
        };
        let id = lock(&database).unwrap().enqueue_job(&job, 0).unwrap();
        let first = timeout(WAIT, events.next()).await.unwrap();
        stop.send(true).unwrap();
        timeout(WAIT, watcher).await.unwrap().unwrap();

        assert!(first.is_some());
        let text = wire(stream::iter(first), 1).await;
        assert!(text.contains("event: job.progress\n"));
        assert!(text.contains(&format!("\"id\":{id}")));
        assert!(timeout(WAIT, events.next()).await.unwrap().is_none());
    }
}

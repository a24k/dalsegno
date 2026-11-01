//! Running queued jobs, one per class at a time.

use dalsegno_app::fetch::{SharedDatabase, lock};
use dalsegno_store::jobs::{JobClass, JobRow};
use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::sleep;

/// How long an idle class waits before looking for work.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// What running a job gives back: nothing, or a message safe to store and show.
pub type JobFuture = Pin<Box<dyn Future<Output = Result<(), String>> + Send>>;

/// Does the work of each kind of job.
pub trait JobHandler: Debug + Send + Sync + 'static {
    /// Runs a job. It can run again after a crash, so it must be safe to repeat (NFR-3).
    fn run(&self, job: JobRow) -> JobFuture;
}

/// Returns the time in epoch milliseconds.
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// Takes jobs from the database and runs them.
#[derive(Debug)]
pub struct JobRunner {
    database: SharedDatabase,
    handler: Arc<dyn JobHandler>,
    poll: Duration,
}

impl JobRunner {
    /// Makes a runner.
    pub fn new(database: SharedDatabase, handler: Arc<dyn JobHandler>) -> Self {
        Self {
            database,
            handler,
            poll: POLL_INTERVAL,
        }
    }

    /// Puts back what a stopped service left running, and returns how many jobs that was.
    pub fn recover(&self) -> Result<usize, String> {
        let database = lock(&self.database).map_err(|error| error.to_string())?;
        database
            .requeue_running_jobs()
            .map_err(|error| error.to_string())
    }

    /// Runs every class until `shutdown` turns true. A job cut off by that stays running, and
    /// the next start puts it back.
    pub async fn run(self: Arc<Self>, shutdown: watch::Receiver<bool>) {
        let mut classes = JoinSet::new();
        for class in JobClass::ALL {
            let runner = Arc::clone(&self);
            classes.spawn(runner.run_class(class, shutdown.clone()));
        }
        classes.join_all().await;
    }

    async fn run_class(self: Arc<Self>, class: JobClass, mut shutdown: watch::Receiver<bool>) {
        while !*shutdown.borrow() {
            match self.claim(class) {
                Some(job) => self.run_one(job, &mut shutdown).await,
                None => {
                    tokio::select! {
                        () = sleep(self.poll) => {}
                        _ = shutdown.changed() => {}
                    }
                }
            }
        }
    }

    fn claim(&self, class: JobClass) -> Option<JobRow> {
        let database = lock(&self.database).ok()?;
        match database.claim_job(class, now_ms()) {
            Ok(job) => job,
            Err(error) => {
                tracing::error!(%error, ?class, "cannot take a job");
                None
            }
        }
    }

    async fn run_one(&self, job: JobRow, shutdown: &mut watch::Receiver<bool>) {
        let id = job.id;
        let mut task = tokio::spawn(self.handler.run(job));
        let outcome = tokio::select! {
            finished = &mut task => finished,
            _ = shutdown.changed() => {
                task.abort();
                return;
            }
        };
        let result = outcome.unwrap_or_else(|_| Err("the job stopped unexpectedly".to_string()));
        if let Err(message) = &result {
            tracing::warn!(id, message, "job failed");
        }
        let finished = lock(&self.database).ok().map(|database| {
            let outcome = result.as_ref().map(|_| ()).map_err(String::as_str);
            database.finish_job(id, outcome, now_ms())
        });
        if let Some(Err(error)) = finished {
            tracing::error!(%error, id, "cannot record the end of a job");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dalsegno_store::database::Database;
    use dalsegno_store::jobs::{JobState, NewJob};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Debug, Default)]
    struct Recorder {
        running: AtomicUsize,
        most: AtomicUsize,
    }

    /// Does what the job's kind says.
    #[derive(Debug)]
    struct Scripted(Arc<Recorder>);

    impl JobHandler for Scripted {
        fn run(&self, job: JobRow) -> JobFuture {
            let recorder = Arc::clone(&self.0);
            Box::pin(async move {
                let running = recorder.running.fetch_add(1, Ordering::SeqCst) + 1;
                recorder.most.fetch_max(running, Ordering::SeqCst);
                sleep(Duration::from_millis(20)).await;
                recorder.running.fetch_sub(1, Ordering::SeqCst);
                match job.job.kind.as_str() {
                    "fails" => Err("no disk".to_string()),
                    "panics" => panic!("a bug"),
                    "hangs" => {
                        sleep(Duration::from_secs(3600)).await;
                        Ok(())
                    }
                    _ => Ok(()),
                }
            })
        }
    }

    fn shared() -> SharedDatabase {
        Arc::new(Mutex::new(Database::open_in_memory().unwrap()))
    }

    fn enqueue(database: &SharedDatabase, kind: &str, class: JobClass) -> i64 {
        let job = NewJob {
            kind: kind.to_string(),
            class,
            target: "{}".to_string(),
        };
        lock(database).unwrap().enqueue_job(&job, 0).unwrap()
    }

    fn state(database: &SharedDatabase, id: i64) -> JobRow {
        lock(database).unwrap().job(id).unwrap().unwrap()
    }

    fn runner(database: &SharedDatabase, recorder: &Arc<Recorder>) -> Arc<JobRunner> {
        Arc::new(JobRunner {
            database: Arc::clone(database),
            handler: Arc::new(Scripted(Arc::clone(recorder))),
            poll: Duration::from_millis(5),
        })
    }

    async fn until(database: &SharedDatabase, id: i64, wanted: JobState) {
        for _ in 0..500 {
            if state(database, id).state == wanted {
                return;
            }
            sleep(Duration::from_millis(5)).await;
        }
        panic!("job {id} never reached {wanted:?}");
    }

    #[tokio::test]
    async fn a_job_runs_and_is_recorded() {
        let database = shared();
        let id = enqueue(&database, "works", JobClass::Cpu);
        let (stop, shutdown) = watch::channel(false);
        let running = tokio::spawn(runner(&database, &Arc::default()).run(shutdown));

        until(&database, id, JobState::Succeeded).await;
        stop.send(true).unwrap();
        running.await.unwrap();

        assert_eq!(state(&database, id).progress, 1.0);
    }

    #[tokio::test]
    async fn a_failure_is_recorded_with_its_reason() {
        let database = shared();
        let id = enqueue(&database, "fails", JobClass::Cpu);
        let (stop, shutdown) = watch::channel(false);
        let running = tokio::spawn(runner(&database, &Arc::default()).run(shutdown));

        until(&database, id, JobState::Failed).await;
        stop.send(true).unwrap();
        running.await.unwrap();

        assert_eq!(state(&database, id).error.as_deref(), Some("no disk"));
    }

    #[tokio::test]
    async fn a_panic_fails_the_job_and_the_runner_goes_on() {
        let database = shared();
        let broken = enqueue(&database, "panics", JobClass::Cpu);
        let next = enqueue(&database, "works", JobClass::Cpu);
        let (stop, shutdown) = watch::channel(false);
        let running = tokio::spawn(runner(&database, &Arc::default()).run(shutdown));

        until(&database, next, JobState::Succeeded).await;
        stop.send(true).unwrap();
        running.await.unwrap();

        let row = state(&database, broken);
        assert_eq!(row.state, JobState::Failed);
        assert!(!row.error.unwrap().contains("a bug"));
    }

    #[tokio::test]
    async fn one_class_runs_one_job_at_a_time() {
        let database = shared();
        let recorder = Arc::new(Recorder::default());
        let ids: Vec<i64> = (0..3)
            .map(|_| enqueue(&database, "works", JobClass::Cpu))
            .collect();
        let (stop, shutdown) = watch::channel(false);
        let running = tokio::spawn(runner(&database, &recorder).run(shutdown));

        until(&database, ids[2], JobState::Succeeded).await;
        stop.send(true).unwrap();
        running.await.unwrap();

        assert_eq!(recorder.most.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn classes_run_side_by_side() {
        let database = shared();
        let recorder = Arc::new(Recorder::default());
        let ids: Vec<i64> = JobClass::ALL
            .iter()
            .map(|class| enqueue(&database, "works", *class))
            .collect();
        let (stop, shutdown) = watch::channel(false);
        let running = tokio::spawn(runner(&database, &recorder).run(shutdown));

        for id in ids {
            until(&database, id, JobState::Succeeded).await;
        }
        stop.send(true).unwrap();
        running.await.unwrap();

        assert!(recorder.most.load(Ordering::SeqCst) > 1);
    }

    #[tokio::test]
    async fn a_job_cut_off_by_shutdown_returns_at_the_next_start() {
        let database = shared();
        let id = enqueue(&database, "hangs", JobClass::Cpu);
        let recorder = Arc::new(Recorder::default());
        let (stop, shutdown) = watch::channel(false);
        let running = tokio::spawn(runner(&database, &recorder).run(shutdown));
        until(&database, id, JobState::Running).await;

        stop.send(true).unwrap();
        running.await.unwrap();

        assert_eq!(state(&database, id).state, JobState::Running);
        assert_eq!(runner(&database, &recorder).recover().unwrap(), 1);
        assert_eq!(state(&database, id).state, JobState::Queued);
    }
}

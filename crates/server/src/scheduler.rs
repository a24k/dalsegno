//! Queueing the work that recurs.

use crate::runner::now_ms;
use dalsegno_app::fetch::{SharedDatabase, lock};
use dalsegno_store::database::{Database, StoreError};
use dalsegno_store::jobs::{JobClass, NewJob};
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::sleep;

/// The kind of job that applies retention.
pub const SWEEP: &str = "sweep";

const DAY_MS: i64 = 24 * 3600 * 1000;
/// How often the schedule is looked at.
const TICK: Duration = Duration::from_secs(60);

/// Queues what is due, and returns how many jobs that was.
fn enqueue_due(database: &Database, now: i64) -> Result<usize, StoreError> {
    let is_due = database
        .last_job_at(SWEEP)?
        .is_none_or(|last| now - last >= DAY_MS);
    if !is_due {
        return Ok(0);
    }
    let sweep = NewJob {
        kind: SWEEP.to_string(),
        class: JobClass::Cpu,
        target: "{}".to_string(),
    };
    Ok(database.enqueue_job_once(&sweep, now)?.into_iter().count())
}

/// Looks at the schedule until `shutdown` turns true.
pub async fn run_scheduler(database: SharedDatabase, mut shutdown: watch::Receiver<bool>) {
    while !*shutdown.borrow() {
        let queued = lock(&database)
            .map_err(|error| error.to_string())
            .and_then(|database| {
                enqueue_due(&database, now_ms()).map_err(|error| error.to_string())
            });
        if let Err(error) = queued {
            tracing::error!(error, "cannot look at the schedule");
        }
        tokio::select! {
            () = sleep(TICK) => {}
            _ = shutdown.changed() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_sweep_is_due_at_once() {
        let database = Database::open_in_memory().unwrap();

        assert_eq!(enqueue_due(&database, 1_000).unwrap(), 1);
    }

    #[test]
    fn a_sweep_is_not_due_again_within_a_day() {
        let database = Database::open_in_memory().unwrap();
        enqueue_due(&database, 0).unwrap();

        assert_eq!(enqueue_due(&database, DAY_MS - 1).unwrap(), 0);
    }

    #[test]
    fn a_sweep_is_due_a_day_later_once_the_last_has_finished() {
        let database = Database::open_in_memory().unwrap();
        enqueue_due(&database, 0).unwrap();
        let job = database.claim_job(JobClass::Cpu, 0).unwrap().unwrap();
        database.finish_job(job.id, Ok(()), 1).unwrap();

        assert_eq!(enqueue_due(&database, DAY_MS).unwrap(), 1);
    }

    #[test]
    fn a_sweep_still_waiting_is_not_queued_twice() {
        let database = Database::open_in_memory().unwrap();
        enqueue_due(&database, 0).unwrap();

        assert_eq!(enqueue_due(&database, DAY_MS).unwrap(), 0);
    }
}

//! Queueing the work that recurs.

use crate::runner::now_ms;
use dalsegno_app::fetch::{SharedDatabase, lock};
use dalsegno_app::lifecycle::StreamTarget;
use dalsegno_store::database::{Database, StoreError};
use dalsegno_store::jobs::{JobClass, NewJob};
use dalsegno_store::streams::{StreamRow, StreamState};
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::sleep;

/// The kind of job that applies retention.
pub const SWEEP: &str = "sweep";

const HOUR_MS: i64 = 3600 * 1000;
const DAY_MS: i64 = 24 * HOUR_MS;
/// How often the schedule is looked at.
const TICK: Duration = Duration::from_secs(60);

/// Queues a job unless it ran within `every`, and returns how many jobs that was.
fn enqueue_if_due(
    database: &Database,
    job: &NewJob,
    every: i64,
    now: i64,
) -> Result<usize, StoreError> {
    let is_due = database
        .last_job_at(&job.kind, &job.target)?
        .is_none_or(|last| now - last >= every);
    if !is_due {
        return Ok(0);
    }
    Ok(database.enqueue_job_once(job, now)?.into_iter().count())
}

/// Queues the hourly work of one stream. Fragments that kept failing stay in open gaps, so a
/// later backfill tries them again while the DVR holds them.
fn enqueue_stream_work(database: &Database, row: &StreamRow, now: i64) -> Result<usize, String> {
    let to_text = |error: StoreError| error.to_string();
    let features = StreamTarget::features(row).map_err(|error| error.to_string())?;
    let mut queued = enqueue_if_due(database, &features, HOUR_MS, now).map_err(to_text)?;
    let has_gaps = !database
        .open_gaps(row.id, row.stream.audio_itag)
        .map_err(to_text)?
        .is_empty();
    if has_gaps {
        let backfill = StreamTarget::backfill(row).map_err(|error| error.to_string())?;
        queued += enqueue_if_due(database, &backfill, HOUR_MS, now).map_err(to_text)?;
    }
    Ok(queued)
}

/// Queues what is due, and returns how many jobs that was.
fn enqueue_due(database: &Database, now: i64) -> Result<usize, String> {
    let sweep = NewJob {
        kind: SWEEP.to_string(),
        class: JobClass::Cpu,
        target: "{}".to_string(),
    };
    let swept = enqueue_if_due(database, &sweep, DAY_MS, now).map_err(|error| error.to_string())?;
    let rows = database
        .streams_in_states(&[StreamState::Backfilling, StreamState::Recording])
        .map_err(|error| error.to_string())?;
    rows.iter().try_fold(swept, |queued, row| {
        Ok(queued + enqueue_stream_work(database, row, now)?)
    })
}

/// Looks at the schedule until `shutdown` turns true.
pub async fn run_scheduler(database: SharedDatabase, mut shutdown: watch::Receiver<bool>) {
    while !*shutdown.borrow() {
        let queued = lock(&database)
            .map_err(|error| error.to_string())
            .and_then(|database| enqueue_due(&database, now_ms()));
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
    use dalsegno_store::streams::NewStream;

    fn with_stream(state: StreamState) -> (Database, i64) {
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
        (database, id)
    }

    fn finish_all(database: &Database) {
        for class in JobClass::ALL {
            while let Some(job) = database.claim_job(class, 0).unwrap() {
                database.finish_job(job.id, Ok(()), 0).unwrap();
            }
        }
    }

    #[test]
    fn a_recording_stream_gets_its_features_every_hour() {
        let (database, _) = with_stream(StreamState::Recording);

        assert_eq!(enqueue_due(&database, 0).unwrap(), 2);
        finish_all(&database);

        assert_eq!(enqueue_due(&database, HOUR_MS - 1).unwrap(), 0);
        assert_eq!(enqueue_due(&database, HOUR_MS).unwrap(), 1);
    }

    #[test]
    fn a_stream_with_open_gaps_is_backfilled() {
        let (database, id) = with_stream(StreamState::Recording);
        database.add_gap(id, 140, 1, 2).unwrap();

        assert_eq!(enqueue_due(&database, 0).unwrap(), 3);
    }

    #[test]
    fn a_paused_stream_gets_no_work() {
        let (database, id) = with_stream(StreamState::Paused);
        database.add_gap(id, 140, 1, 2).unwrap();

        assert_eq!(enqueue_due(&database, 0).unwrap(), 1);
    }

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

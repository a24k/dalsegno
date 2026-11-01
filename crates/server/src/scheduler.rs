//! Queueing the work that recurs.

use crate::runner::now_ms;
use dalsegno_app::fetch::{SharedDatabase, lock};
use dalsegno_app::lifecycle::StreamTarget;
use dalsegno_app::notify::DEADLINE;
use dalsegno_store::database::{Database, StoreError};
use dalsegno_store::jobs::{JobClass, NewJob};
use dalsegno_store::streams::{StreamRow, StreamState};
use serde_json::json;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::sleep;

/// The kind of job that applies retention.
pub const SWEEP: &str = "sweep";

const HOUR_MS: i64 = 3600 * 1000;
const DAY_MS: i64 = 24 * HOUR_MS;
const DEADLINE_WARNING_MS: i64 = 24 * HOUR_MS;
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
    let analysis = StreamTarget::analysis(row).map_err(|error| error.to_string())?;
    let mut queued = enqueue_if_due(database, &features, HOUR_MS, now).map_err(to_text)?;
    queued += enqueue_if_due(database, &analysis, HOUR_MS, now).map_err(to_text)?;
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

/// Warns once about each change whose old version is about to leave the DVR (FR-J3).
fn warn_of_deadlines(database: &Database, now: i64) -> Result<(), StoreError> {
    for change in database.changes_near_deadline(now + DEADLINE_WARNING_MS)? {
        let body = json!({ "stream": change.title, "dvr_deadline_at": change.deadline_at });
        let subject = format!("change:{}:deadline", change.id);
        database.enqueue_notification(DEADLINE, &subject, &body.to_string(), now)?;
    }
    Ok(())
}

/// Queues what is due, and returns how many jobs that was.
fn enqueue_due(database: &Database, now: i64) -> Result<usize, String> {
    let sweep = NewJob {
        kind: SWEEP.to_string(),
        class: JobClass::Cpu,
        target: "{}".to_string(),
    };
    let swept = enqueue_if_due(database, &sweep, DAY_MS, now).map_err(|error| error.to_string())?;
    warn_of_deadlines(database, now).map_err(|error| error.to_string())?;
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
    fn a_recording_stream_is_analysed_every_hour() {
        let (database, _) = with_stream(StreamState::Recording);

        assert_eq!(enqueue_due(&database, 0).unwrap(), 3);
        finish_all(&database);

        assert_eq!(enqueue_due(&database, HOUR_MS - 1).unwrap(), 0);
        assert_eq!(enqueue_due(&database, HOUR_MS).unwrap(), 2);
    }

    #[test]
    fn a_stream_with_open_gaps_is_backfilled() {
        let (database, id) = with_stream(StreamState::Recording);
        database.add_gap(id, 140, 1, 2).unwrap();

        assert_eq!(enqueue_due(&database, 0).unwrap(), 4);
    }

    fn with_change(deadline: i64) -> Database {
        let (database, stream) = with_stream(StreamState::Recording);
        database
            .connection()
            .execute_batch(
                "INSERT INTO programs (id, short_name, name_template, addition_suffix, output_dir)
                 VALUES (1, 'a', 'a', 'a', 'a');",
            )
            .unwrap();
        database
            .connection()
            .execute(
                "INSERT INTO changes (program_id, stream_id, detected_at, state, dvr_deadline_at,
                 start_time, last_signal_time, last_delta)
                 VALUES (1, ?1, 0, 'confirmed', ?2, 0, 0, 0)",
                (stream, deadline),
            )
            .unwrap();
        database
    }

    fn notifications(database: &Database) -> usize {
        database.pending_notifications(10).unwrap().len()
    }

    #[test]
    fn a_near_deadline_is_warned_of_once() {
        let database = with_change(DEADLINE_WARNING_MS);

        enqueue_due(&database, 0).unwrap();
        enqueue_due(&database, 60_000).unwrap();

        assert_eq!(notifications(&database), 1);
    }

    #[test]
    fn a_far_deadline_is_not_yet_warned_of() {
        let database = with_change(DEADLINE_WARNING_MS + 1);

        enqueue_due(&database, 0).unwrap();

        assert_eq!(notifications(&database), 0);
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

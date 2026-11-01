//! Starting and stopping streams (FR-S4, FR-S6).
//!
//! These only change the database, and the service acts on what they leave there, so they work
//! with the service running or not.

use crate::acquisition::Library;
use crate::error::AcquireError;
use crate::fetch::lock;
use dalsegno_store::database::Database;
use dalsegno_store::jobs::{JobClass, NewJob};
use dalsegno_store::streams::{StreamRow, StreamState};
use serde::{Deserialize, Serialize};

/// The job that fills a stream's gaps.
pub const BACKFILL: &str = "backfill";
/// The job that brings a stream's envelope up to date.
pub const FEATURES: &str = "features";
/// The job that tracks a stream and follows its changes.
pub const ANALYSIS: &str = "analysis";
/// The job that fetches the video around a change.
pub const PREFETCH: &str = "prefetch";

/// What a job about a stream is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamTarget {
    /// Video ID.
    pub stream: String,
}

impl StreamTarget {
    fn job(row: &StreamRow, kind: &str, class: JobClass) -> Result<NewJob, AcquireError> {
        let target = Self {
            stream: row.stream.video_id.clone(),
        };
        Ok(NewJob {
            kind: kind.to_string(),
            class,
            target: serde_json::to_string(&target).map_err(|_| AcquireError::Task)?,
        })
    }

    /// Returns the job that fills a stream's gaps.
    pub fn backfill(row: &StreamRow) -> Result<NewJob, AcquireError> {
        Self::job(row, BACKFILL, JobClass::NetLow)
    }

    /// Returns the job that brings a stream's envelope up to date.
    pub fn features(row: &StreamRow) -> Result<NewJob, AcquireError> {
        Self::job(row, FEATURES, JobClass::Cpu)
    }

    /// Returns the job that tracks a stream.
    pub fn analysis(row: &StreamRow) -> Result<NewJob, AcquireError> {
        Self::job(row, ANALYSIS, JobClass::Cpu)
    }

    /// Reads a stored target.
    pub fn parse(target: &str) -> Option<Self> {
        serde_json::from_str(target).ok()
    }
}

/// The video to keep and fetch for a change (FR-E1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrefetchTarget {
    /// Video ID.
    pub stream: String,
    /// The change that holds it.
    pub change: i64,
    /// First `sq`.
    pub first: u64,
    /// Last `sq`.
    pub last: u64,
}

impl PrefetchTarget {
    /// Returns the job.
    pub fn job(&self) -> Result<NewJob, AcquireError> {
        Ok(NewJob {
            kind: PREFETCH.to_string(),
            class: JobClass::NetLow,
            target: serde_json::to_string(self).map_err(|_| AcquireError::Task)?,
        })
    }

    /// Reads a stored target.
    pub fn parse(target: &str) -> Option<Self> {
        serde_json::from_str(target).ok()
    }
}

fn current_state(database: &Database, row: &StreamRow) -> Result<StreamState, AcquireError> {
    Ok(database
        .stream_state(row.id)?
        .map_or(StreamState::Registered, |(state, _)| state))
}

impl Library {
    /// Starts following a stream and filling what it missed. Returns the state it is now in.
    pub fn start_stream(&self, stream: &str, now: i64) -> Result<StreamState, AcquireError> {
        let row = self.registered(stream)?;
        let database = lock(&self.database)?;
        match current_state(&database, &row)? {
            StreamState::Ended => Err(AcquireError::InvalidRequest("the stream has ended")),
            state @ (StreamState::Backfilling | StreamState::Recording) => Ok(state),
            StreamState::Registered | StreamState::Paused | StreamState::Error => {
                database.set_stream_state(row.id, StreamState::Backfilling, None)?;
                database.enqueue_job_once(&StreamTarget::backfill(&row)?, now)?;
                Ok(StreamState::Backfilling)
            }
        }
    }

    /// Stops following a stream. Returns the state it is now in.
    pub fn stop_stream(&self, stream: &str) -> Result<StreamState, AcquireError> {
        let row = self.registered(stream)?;
        let database = lock(&self.database)?;
        match current_state(&database, &row)? {
            StreamState::Backfilling | StreamState::Recording | StreamState::Error => {
                database.set_stream_state(row.id, StreamState::Paused, None)?;
                Ok(StreamState::Paused)
            }
            state @ (StreamState::Registered | StreamState::Paused | StreamState::Ended) => {
                Ok(state)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dalsegno_store::jobs::JobClass;
    use dalsegno_store::streams::NewStream;
    use tempfile::{TempDir, tempdir};

    const VIDEO: &str = "abcDEF123_-";

    fn library() -> (TempDir, Library, i64) {
        let root = tempdir().unwrap();
        let library = Library::open(root.path()).unwrap();
        let id = lock(&library.database)
            .unwrap()
            .insert_stream(&NewStream {
                video_id: VIDEO.to_string(),
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
        (root, library, id)
    }

    fn state(library: &Library, id: i64) -> StreamState {
        lock(&library.database)
            .unwrap()
            .stream_state(id)
            .unwrap()
            .unwrap()
            .0
    }

    fn set(library: &Library, id: i64, state: StreamState) {
        lock(&library.database)
            .unwrap()
            .set_stream_state(id, state, Some("a reason"))
            .unwrap();
    }

    #[test]
    fn starting_queues_a_backfill() {
        let (_root, library, id) = library();

        assert_eq!(
            library.start_stream(VIDEO, 5).unwrap(),
            StreamState::Backfilling
        );

        assert_eq!(state(&library, id), StreamState::Backfilling);
        let database = lock(&library.database).unwrap();
        let job = database.claim_job(JobClass::NetLow, 6).unwrap().unwrap();
        assert_eq!(job.job.kind, BACKFILL);
        assert_eq!(
            StreamTarget::parse(&job.job.target).unwrap().stream,
            VIDEO.to_string()
        );
    }

    #[test]
    fn starting_a_running_stream_changes_nothing() {
        let (_root, library, id) = library();
        set(&library, id, StreamState::Recording);

        assert_eq!(
            library.start_stream(VIDEO, 5).unwrap(),
            StreamState::Recording
        );

        let database = lock(&library.database).unwrap();
        assert_eq!(database.claim_job(JobClass::NetLow, 6).unwrap(), None);
    }

    #[test]
    fn starting_clears_the_reason_of_an_error() {
        let (_root, library, id) = library();
        set(&library, id, StreamState::Error);

        library.start_stream(VIDEO, 5).unwrap();

        let database = lock(&library.database).unwrap();
        assert_eq!(database.stream_state(id).unwrap().unwrap().1, None);
    }

    #[test]
    fn a_finished_stream_cannot_start() {
        let (_root, library, id) = library();
        set(&library, id, StreamState::Ended);

        assert!(matches!(
            library.start_stream(VIDEO, 5),
            Err(AcquireError::InvalidRequest(_))
        ));
    }

    #[test]
    fn stopping_pauses_a_running_stream() {
        let (_root, library, id) = library();
        set(&library, id, StreamState::Recording);

        assert_eq!(library.stop_stream(VIDEO).unwrap(), StreamState::Paused);
        assert_eq!(state(&library, id), StreamState::Paused);
    }

    #[test]
    fn stopping_a_stream_never_started_changes_nothing() {
        let (_root, library, id) = library();

        assert_eq!(library.stop_stream(VIDEO).unwrap(), StreamState::Registered);
        assert_eq!(state(&library, id), StreamState::Registered);
    }

    #[test]
    fn a_stream_not_registered_cannot_start() {
        let (_root, library, _) = library();

        assert!(matches!(
            library.start_stream("zzzZZZ999_-", 5),
            Err(AcquireError::NotRegistered(_))
        ));
    }
}

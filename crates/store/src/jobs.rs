//! The job queue (FR-J1). Jobs survive a restart, and each one is safe to run again (NFR-3).

use crate::database::{Database, StoreError};
use rusqlite::{OptionalExtension, Row, params};

/// A resource that jobs take turns on. Each class runs one job at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobClass {
    /// Backfill and range fetches.
    NetLow,
    /// Features, analysis and comparison.
    Cpu,
    /// Candidates, assembly and previews.
    Ffmpeg,
    /// Library transfer and Plex.
    Transfer,
}

impl JobClass {
    /// Every class.
    pub const ALL: [Self; 4] = [Self::NetLow, Self::Cpu, Self::Ffmpeg, Self::Transfer];

    fn as_str(self) -> &'static str {
        match self {
            Self::NetLow => "net-low",
            Self::Cpu => "cpu",
            Self::Ffmpeg => "ffmpeg",
            Self::Transfer => "transfer",
        }
    }

    fn parse(text: &str) -> Result<Self, StoreError> {
        Self::ALL
            .into_iter()
            .find(|class| class.as_str() == text)
            .ok_or(StoreError::Invalid("job class"))
    }
}

/// Where a job is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    /// Waiting for a slot.
    Queued,
    /// Taken by the runner.
    Running,
    /// Finished.
    Succeeded,
    /// Finished with an error.
    Failed,
    /// Stopped by a person.
    Cancelled,
}

impl JobState {
    fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(StoreError::Invalid("job state")),
        }
    }
}

/// A job to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    /// What to do.
    pub kind: String,
    /// The resource it takes.
    pub class: JobClass,
    /// JSON naming what to do it to.
    pub target: String,
}

/// A stored job.
#[derive(Debug, Clone, PartialEq)]
pub struct JobRow {
    /// Row ID.
    pub id: i64,
    /// What to do.
    pub job: NewJob,
    /// Where it is.
    pub state: JobState,
    /// Share done, from 0 to 1.
    pub progress: f64,
    /// JSON a job resumes from.
    pub checkpoint: Option<String>,
    /// Why it failed.
    pub error: Option<String>,
    /// When it was queued.
    pub created_at: i64,
    /// When it was last taken.
    pub started_at: Option<i64>,
    /// When it finished.
    pub finished_at: Option<i64>,
}

const COLUMNS: &str = "id, kind, class, target, state, progress, checkpoint, error, created_at, \
                       started_at, finished_at";

/// A row as stored, whose class and state are checked once SQLite is done with it.
struct RawJob {
    id: i64,
    kind: String,
    class: String,
    target: String,
    state: String,
    progress: f64,
    checkpoint: Option<String>,
    error: Option<String>,
    created_at: i64,
    started_at: Option<i64>,
    finished_at: Option<i64>,
}

fn read_job(row: &Row<'_>) -> Result<RawJob, rusqlite::Error> {
    Ok(RawJob {
        id: row.get(0)?,
        kind: row.get(1)?,
        class: row.get(2)?,
        target: row.get(3)?,
        state: row.get(4)?,
        progress: row.get(5)?,
        checkpoint: row.get(6)?,
        error: row.get(7)?,
        created_at: row.get(8)?,
        started_at: row.get(9)?,
        finished_at: row.get(10)?,
    })
}

fn checked(raw: RawJob) -> Result<JobRow, StoreError> {
    Ok(JobRow {
        id: raw.id,
        job: NewJob {
            kind: raw.kind,
            class: JobClass::parse(&raw.class)?,
            target: raw.target,
        },
        state: JobState::parse(&raw.state)?,
        progress: raw.progress,
        checkpoint: raw.checkpoint,
        error: raw.error,
        created_at: raw.created_at,
        started_at: raw.started_at,
        finished_at: raw.finished_at,
    })
}

impl Database {
    /// Queues a job, and returns its row ID.
    pub fn enqueue_job(&self, job: &NewJob, now: i64) -> Result<i64, StoreError> {
        self.connection.execute(
            "INSERT INTO jobs (kind, class, target, state, created_at) \
             VALUES (?1, ?2, ?3, 'queued', ?4)",
            params![job.kind, job.class.as_str(), job.target, now],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Queues a job unless the same one is waiting or running. Returns its row ID if it did.
    pub fn enqueue_job_once(&self, job: &NewJob, now: i64) -> Result<Option<i64>, StoreError> {
        let added = self.connection.execute(
            "INSERT INTO jobs (kind, class, target, state, created_at) \
             SELECT ?1, ?2, ?3, 'queued', ?4 WHERE NOT EXISTS ( \
               SELECT 1 FROM jobs WHERE kind = ?1 AND target = ?3 \
               AND state IN ('queued', 'running'))",
            params![job.kind, job.class.as_str(), job.target, now],
        )?;
        Ok((added > 0).then(|| self.connection.last_insert_rowid()))
    }

    /// Takes the oldest waiting job of a class.
    pub fn claim_job(&self, class: JobClass, now: i64) -> Result<Option<JobRow>, StoreError> {
        let id: Option<i64> = self
            .connection
            .query_row(
                "UPDATE jobs SET state = 'running', started_at = ?2 WHERE id = ( \
                   SELECT id FROM jobs WHERE state = 'queued' AND class = ?1 \
                   ORDER BY id LIMIT 1) RETURNING id",
                params![class.as_str(), now],
                |row| row.get(0),
            )
            .optional()?;
        id.map_or(Ok(None), |id| self.job(id))
    }

    /// Returns a job.
    pub fn job(&self, id: i64) -> Result<Option<JobRow>, StoreError> {
        let sql = format!("SELECT {COLUMNS} FROM jobs WHERE id = ?1");
        self.connection
            .query_row(&sql, params![id], read_job)
            .optional()?
            .map(checked)
            .transpose()
    }

    /// Returns a page of jobs, newest first.
    pub fn list_jobs(&self, limit: u32, offset: u32) -> Result<Vec<JobRow>, StoreError> {
        let sql = format!("SELECT {COLUMNS} FROM jobs ORDER BY id DESC LIMIT ?1 OFFSET ?2");
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params![limit, offset], read_job)?;
        rows.map(|row| checked(row?)).collect()
    }

    /// Records how far a running job has come, and what it would resume from.
    pub fn set_job_progress(
        &self,
        id: i64,
        progress: f64,
        checkpoint: Option<&str>,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE jobs SET progress = ?2, checkpoint = COALESCE(?3, checkpoint) \
             WHERE id = ?1 AND state = 'running'",
            params![id, progress, checkpoint],
        )?;
        Ok(())
    }

    /// Records how a running job ended. A job cancelled meanwhile stays cancelled.
    pub fn finish_job(
        &self,
        id: i64,
        outcome: Result<(), &str>,
        now: i64,
    ) -> Result<(), StoreError> {
        let (state, progress) = match outcome {
            Ok(()) => ("succeeded", Some(1.0)),
            Err(_) => ("failed", None),
        };
        self.connection.execute(
            "UPDATE jobs SET state = ?2, progress = COALESCE(?3, progress), error = ?4, \
             finished_at = ?5 WHERE id = ?1 AND state = 'running'",
            params![id, state, progress, outcome.err(), now],
        )?;
        Ok(())
    }

    /// Cancels a job that has not finished, and returns whether there was one.
    pub fn cancel_job(&self, id: i64, now: i64) -> Result<bool, StoreError> {
        let changed = self.connection.execute(
            "UPDATE jobs SET state = 'cancelled', finished_at = ?2 \
             WHERE id = ?1 AND state IN ('queued', 'running')",
            params![id, now],
        )?;
        Ok(changed > 0)
    }

    /// Puts jobs a stopped service left running back in the queue, keeping their checkpoints.
    pub fn requeue_running_jobs(&self) -> Result<usize, StoreError> {
        Ok(self.connection.execute(
            "UPDATE jobs SET state = 'queued', started_at = NULL WHERE state = 'running'",
            [],
        )?)
    }

    /// Returns when a job of this kind was last queued.
    pub fn last_job_at(&self, kind: &str) -> Result<Option<i64>, StoreError> {
        Ok(self.connection.query_row(
            "SELECT MAX(created_at) FROM jobs WHERE kind = ?1",
            params![kind],
            |row| row.get(0),
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Database {
        Database::open_in_memory().unwrap()
    }

    fn job(kind: &str, class: JobClass) -> NewJob {
        NewJob {
            kind: kind.to_string(),
            class,
            target: "{}".to_string(),
        }
    }

    #[test]
    fn a_queued_job_can_be_read_back() {
        let database = database();

        let id = database
            .enqueue_job(&job("sweep", JobClass::Cpu), 5)
            .unwrap();

        let row = database.job(id).unwrap().unwrap();
        assert_eq!(row.job, job("sweep", JobClass::Cpu));
        assert_eq!(row.state, JobState::Queued);
        assert_eq!(row.created_at, 5);
    }

    #[test]
    fn every_class_survives_being_stored() {
        let database = database();

        for class in JobClass::ALL {
            let id = database.enqueue_job(&job("a", class), 0).unwrap();

            assert_eq!(database.job(id).unwrap().unwrap().job.class, class);
        }
    }

    #[test]
    fn the_oldest_job_of_a_class_is_taken_first() {
        let database = database();
        let first = database.enqueue_job(&job("a", JobClass::Cpu), 0).unwrap();
        database.enqueue_job(&job("b", JobClass::Cpu), 1).unwrap();

        let claimed = database.claim_job(JobClass::Cpu, 9).unwrap().unwrap();

        assert_eq!(claimed.id, first);
        assert_eq!(claimed.state, JobState::Running);
        assert_eq!(claimed.started_at, Some(9));
    }

    #[test]
    fn a_job_is_taken_once() {
        let database = database();
        database.enqueue_job(&job("a", JobClass::Cpu), 0).unwrap();

        database.claim_job(JobClass::Cpu, 1).unwrap();

        assert_eq!(database.claim_job(JobClass::Cpu, 2).unwrap(), None);
    }

    #[test]
    fn a_class_takes_only_its_own_jobs() {
        let database = database();
        database
            .enqueue_job(&job("a", JobClass::Ffmpeg), 0)
            .unwrap();

        assert_eq!(database.claim_job(JobClass::Cpu, 1).unwrap(), None);
    }

    #[test]
    fn the_same_waiting_job_is_not_queued_twice() {
        let database = database();
        let wanted = job("sweep", JobClass::Cpu);

        let first = database.enqueue_job_once(&wanted, 0).unwrap();
        let second = database.enqueue_job_once(&wanted, 1).unwrap();

        assert!(first.is_some());
        assert_eq!(second, None);
    }

    #[test]
    fn the_same_job_can_run_again_once_finished() {
        let database = database();
        let wanted = job("sweep", JobClass::Cpu);
        let id = database.enqueue_job_once(&wanted, 0).unwrap().unwrap();
        database.claim_job(JobClass::Cpu, 1).unwrap();
        database.finish_job(id, Ok(()), 2).unwrap();

        assert!(database.enqueue_job_once(&wanted, 3).unwrap().is_some());
    }

    #[test]
    fn success_completes_the_progress() {
        let database = database();
        let id = database.enqueue_job(&job("a", JobClass::Cpu), 0).unwrap();
        database.claim_job(JobClass::Cpu, 1).unwrap();

        database.finish_job(id, Ok(()), 2).unwrap();

        let row = database.job(id).unwrap().unwrap();
        assert_eq!(row.state, JobState::Succeeded);
        assert_eq!(row.progress, 1.0);
        assert_eq!(row.finished_at, Some(2));
    }

    #[test]
    fn failure_keeps_the_reason_and_the_progress() {
        let database = database();
        let id = database.enqueue_job(&job("a", JobClass::Cpu), 0).unwrap();
        database.claim_job(JobClass::Cpu, 1).unwrap();
        database.set_job_progress(id, 0.5, None).unwrap();

        database.finish_job(id, Err("no disk"), 2).unwrap();

        let row = database.job(id).unwrap().unwrap();
        assert_eq!(row.state, JobState::Failed);
        assert_eq!(row.error.as_deref(), Some("no disk"));
        assert_eq!(row.progress, 0.5);
    }

    #[test]
    fn a_job_cancelled_while_running_stays_cancelled() {
        let database = database();
        let id = database.enqueue_job(&job("a", JobClass::Cpu), 0).unwrap();
        database.claim_job(JobClass::Cpu, 1).unwrap();

        assert!(database.cancel_job(id, 2).unwrap());
        database.finish_job(id, Ok(()), 3).unwrap();

        let row = database.job(id).unwrap().unwrap();
        assert_eq!(row.state, JobState::Cancelled);
        assert_eq!(row.finished_at, Some(2));
    }

    #[test]
    fn a_finished_job_cannot_be_cancelled() {
        let database = database();
        let id = database.enqueue_job(&job("a", JobClass::Cpu), 0).unwrap();
        database.claim_job(JobClass::Cpu, 1).unwrap();
        database.finish_job(id, Ok(()), 2).unwrap();

        assert!(!database.cancel_job(id, 3).unwrap());
    }

    #[test]
    fn a_running_job_goes_back_in_the_queue_with_its_checkpoint() {
        let database = database();
        let id = database.enqueue_job(&job("a", JobClass::Cpu), 0).unwrap();
        database.claim_job(JobClass::Cpu, 1).unwrap();
        database
            .set_job_progress(id, 0.4, Some("{\"sq\":7}"))
            .unwrap();

        assert_eq!(database.requeue_running_jobs().unwrap(), 1);

        let row = database.job(id).unwrap().unwrap();
        assert_eq!(row.state, JobState::Queued);
        assert_eq!(row.started_at, None);
        assert_eq!(row.checkpoint.as_deref(), Some("{\"sq\":7}"));
        assert_eq!(row.progress, 0.4);
    }

    #[test]
    fn progress_without_a_checkpoint_keeps_the_last_one() {
        let database = database();
        let id = database.enqueue_job(&job("a", JobClass::Cpu), 0).unwrap();
        database.claim_job(JobClass::Cpu, 1).unwrap();
        database.set_job_progress(id, 0.1, Some("{}")).unwrap();

        database.set_job_progress(id, 0.2, None).unwrap();

        assert_eq!(
            database.job(id).unwrap().unwrap().checkpoint.as_deref(),
            Some("{}")
        );
    }

    #[test]
    fn jobs_are_listed_newest_first() {
        let database = database();
        let first = database.enqueue_job(&job("a", JobClass::Cpu), 0).unwrap();
        let second = database.enqueue_job(&job("b", JobClass::Cpu), 1).unwrap();

        let ids: Vec<i64> = database
            .list_jobs(10, 0)
            .unwrap()
            .iter()
            .map(|row| row.id)
            .collect();

        assert_eq!(ids, [second, first]);
        assert_eq!(database.list_jobs(1, 1).unwrap()[0].id, first);
    }

    #[test]
    fn the_last_job_of_a_kind_is_found() {
        let database = database();
        assert_eq!(database.last_job_at("sweep").unwrap(), None);
        database
            .enqueue_job(&job("sweep", JobClass::Cpu), 4)
            .unwrap();
        database
            .enqueue_job(&job("sweep", JobClass::Cpu), 8)
            .unwrap();

        assert_eq!(database.last_job_at("sweep").unwrap(), Some(8));
    }

    #[test]
    fn a_stored_class_this_program_does_not_know_is_an_error() {
        let database = database();
        let id = database.enqueue_job(&job("a", JobClass::Cpu), 0).unwrap();
        database
            .connection()
            .execute("UPDATE jobs SET class = 'quantum'", [])
            .unwrap();

        assert!(matches!(database.job(id), Err(StoreError::Invalid(_))));
    }
}

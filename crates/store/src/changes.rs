//! Versions, tracking state and changes (D-08, D-25).

use crate::database::{Database, StoreError};
use rusqlite::{OptionalExtension, Row, params};

/// A version to record.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NewVersion {
    /// Program.
    pub program_id: i64,
    /// Stream it was seen on.
    pub stream_id: i64,
    /// Period from audio, in seconds.
    pub period: f64,
    /// Stream time of the start of the held cycle.
    pub anchor_time: f64,
    /// When it was recorded.
    pub seen_at: i64,
    /// Stream time it went on air.
    pub on_air_from: f64,
}

/// A stored version.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VersionRow {
    /// Row ID.
    pub id: i64,
    /// Period from audio, in seconds.
    pub period: f64,
    /// Stream time of the start of the held cycle.
    pub anchor_time: f64,
    /// Stream time it went on air.
    pub on_air_from: f64,
}

/// A change to record.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NewChange {
    /// Program.
    pub program_id: i64,
    /// Stream.
    pub stream_id: i64,
    /// When it was seen.
    pub detected_at: i64,
    /// The version it changes.
    pub old_version_id: i64,
    /// Stream time of the first signal.
    pub start_time: f64,
    /// Net change so far, in seconds.
    pub delta: f64,
    /// When the export of the old version must be done by.
    pub dvr_deadline_at: i64,
}

/// A change that has not closed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpenChange {
    /// Row ID.
    pub id: i64,
    /// The version it changes.
    pub old_version_id: i64,
    /// Stream time of the first signal.
    pub start_time: f64,
    /// Stream time of the latest signal.
    pub last_signal_time: f64,
    /// Net change so far, in seconds.
    pub last_delta: f64,
}

/// A stretch of a compared cycle.
#[derive(Debug, Clone, PartialEq)]
pub struct NewRegion {
    /// `existing`, `new`, `revival`, `known`, `deleted` or `no_data`.
    pub class: String,
    /// Start, in stream time. A deleted region is in the old version's time.
    pub start: f64,
    /// End.
    pub end: f64,
    /// The version it matched.
    pub match_version: Option<i64>,
    /// Where it starts in that version.
    pub match_start: Option<f64>,
}

/// What closing a change records.
#[derive(Debug, Clone, PartialEq)]
pub struct Confirmation {
    /// Row ID of the change.
    pub change_id: i64,
    /// The new version.
    pub new_version_id: i64,
    /// JSON array of self-checks.
    pub checks: String,
    /// JSON array of video boundaries.
    pub boundaries: String,
    /// The comparison.
    pub regions: Vec<NewRegion>,
}

/// A stretch of the lag, with its kind as stored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoredRun {
    /// First point.
    pub start: f64,
    /// Last point.
    pub end: f64,
    /// `lag`, `outlier`, `unmatched` or `no_data`.
    pub kind: &'static str,
    /// The lag.
    pub lag: Option<f64>,
}

/// How far tracking has come.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnalysisState {
    /// Stream time of the last tracking point.
    pub tracked_until: f64,
    /// Whether the next pass searches every lag.
    pub widened: bool,
}

fn version_from_row(row: &Row<'_>) -> rusqlite::Result<VersionRow> {
    Ok(VersionRow {
        id: row.get(0)?,
        period: row.get(1)?,
        anchor_time: row.get(2)?,
        on_air_from: row.get(3)?,
    })
}

impl Database {
    /// Runs `work` in one transaction, so a state change and its outbox row stay together.
    pub fn atomically<T, E: From<StoreError>>(
        &self,
        work: impl FnOnce(&Self) -> Result<T, E>,
    ) -> Result<T, E> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(StoreError::from)?;
        let result = work(self)?;
        transaction.commit().map_err(StoreError::from)?;
        Ok(result)
    }

    /// Records a version, and returns its row ID.
    pub fn insert_version(&self, version: &NewVersion) -> Result<i64, StoreError> {
        self.connection.execute(
            "INSERT INTO versions (program_id, stream_id, period_audio, anchor_time, anchor_at, \
             first_seen_at, on_air_from_time) VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6)",
            params![
                version.program_id,
                version.stream_id,
                version.period,
                version.anchor_time,
                version.seen_at,
                version.on_air_from,
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Returns the version on air.
    pub fn current_version(&self, program_id: i64) -> Result<Option<VersionRow>, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT id, period_audio, anchor_time, on_air_from_time FROM versions \
                 WHERE program_id = ?1 AND on_air_until_time IS NULL ORDER BY id DESC LIMIT 1",
                [program_id],
                version_from_row,
            )
            .optional()?)
    }

    /// Returns a version.
    pub fn version(&self, id: i64) -> Result<Option<VersionRow>, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT id, period_audio, anchor_time, on_air_from_time FROM versions \
                 WHERE id = ?1",
                [id],
                version_from_row,
            )
            .optional()?)
    }

    /// Takes a version off the air. `last_cycle_start` drives its DVR deadline (D-23).
    pub fn supersede_version(
        &self,
        id: i64,
        until: f64,
        last_cycle_start: f64,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE versions SET on_air_until_time = ?2, last_cycle_start_time = ?3 WHERE id = ?1",
            params![id, until, last_cycle_start],
        )?;
        Ok(())
    }

    /// Returns a stream's period, if one has been found.
    pub fn stream_period(&self, stream_id: i64) -> Result<Option<f64>, StoreError> {
        let period: Option<Option<f64>> = self
            .connection
            .query_row(
                "SELECT period FROM streams WHERE id = ?1",
                [stream_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(period.flatten())
    }

    /// Records a stream's period.
    pub fn set_stream_period(&self, stream_id: i64, period: f64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE streams SET period = ?2, testable_period_max = NULL WHERE id = ?1",
            params![stream_id, period],
        )?;
        Ok(())
    }

    /// Records the longest period the history could have shown (D-03).
    pub fn set_testable_period(&self, stream_id: i64, seconds: f64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE streams SET testable_period_max = ?2 WHERE id = ?1",
            params![stream_id, seconds],
        )?;
        Ok(())
    }

    /// Returns how far tracking has come.
    pub fn analysis_state(&self, stream_id: i64) -> Result<Option<AnalysisState>, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT tracked_until, search_widened FROM analysis_state WHERE stream_id = ?1",
                [stream_id],
                |row| {
                    Ok(AnalysisState {
                        tracked_until: row.get(0)?,
                        widened: row.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    /// Records a tracking pass: its runs replace those from `from` on, so overlap is safe.
    pub fn save_tracking(
        &self,
        stream_id: i64,
        from: f64,
        runs: &[StoredRun],
        state: AnalysisState,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "DELETE FROM lag_runs WHERE stream_id = ?1 AND start_time >= ?2",
            params![stream_id, from],
        )?;
        for run in runs {
            self.connection.execute(
                "INSERT INTO lag_runs (stream_id, start_time, end_time, kind, lag) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![stream_id, run.start, run.end, run.kind, run.lag],
            )?;
        }
        self.connection.execute(
            "INSERT INTO analysis_state (stream_id, tracked_until, search_widened) \
             VALUES (?1, ?2, ?3) ON CONFLICT (stream_id) DO UPDATE SET \
             tracked_until = excluded.tracked_until, search_widened = excluded.search_widened",
            params![stream_id, state.tracked_until, state.widened],
        )?;
        Ok(())
    }

    /// Opens a change, and returns its row ID. A program has at most one open at a time.
    pub fn insert_change(&self, change: &NewChange) -> Result<i64, StoreError> {
        self.connection.execute(
            "INSERT INTO changes (program_id, stream_id, detected_at, old_version_id, state, \
             dvr_deadline_at, start_time, last_signal_time, last_delta) \
             VALUES (?1, ?2, ?3, ?4, 'provisional', ?5, ?6, ?6, ?7)",
            params![
                change.program_id,
                change.stream_id,
                change.detected_at,
                change.old_version_id,
                change.dvr_deadline_at,
                change.start_time,
                change.delta,
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Returns a program's open change.
    pub fn open_change(&self, program_id: i64) -> Result<Option<OpenChange>, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT id, old_version_id, start_time, last_signal_time, last_delta \
                 FROM changes WHERE program_id = ?1 AND state = 'provisional'",
                [program_id],
                |row| {
                    Ok(OpenChange {
                        id: row.get(0)?,
                        old_version_id: row.get(1)?,
                        start_time: row.get(2)?,
                        last_signal_time: row.get(3)?,
                        last_delta: row.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    /// Records a later signal of an open change.
    pub fn update_change_signal(&self, id: i64, time: f64, delta: f64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE changes SET last_signal_time = ?2, last_delta = ?3 WHERE id = ?1",
            params![id, time, delta],
        )?;
        Ok(())
    }

    /// Closes a change with its comparison.
    pub fn confirm_change(&self, confirmation: &Confirmation) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE changes SET state = 'confirmed', new_version_id = ?2, checks = ?3, \
             boundaries = ?4 WHERE id = ?1 AND state = 'provisional'",
            params![
                confirmation.change_id,
                confirmation.new_version_id,
                confirmation.checks,
                confirmation.boundaries,
            ],
        )?;
        for region in &confirmation.regions {
            self.connection.execute(
                "INSERT INTO change_regions (change_id, class, start_time, end_time, \
                 match_version_id, match_start_time) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    confirmation.change_id,
                    region.class,
                    region.start,
                    region.end,
                    region.match_version,
                    region.match_start,
                ],
            )?;
        }
        Ok(())
    }

    /// Writes a notification to the outbox, once per kind and subject (D-25).
    pub fn enqueue_notification(
        &self,
        kind: &str,
        subject: &str,
        body: &str,
        now: i64,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT OR IGNORE INTO notifications (kind, subject, body, created_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![kind, subject, body, now],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::programs::NewProgram;
    use crate::streams::tests::new_stream;

    fn database() -> (Database, i64, i64) {
        let database = Database::open_in_memory().unwrap();
        let stream = database.insert_stream(&new_stream("abcDEF123_-")).unwrap();
        let program = database
            .insert_program(&NewProgram {
                short_name: "Example TV".to_string(),
                channel_id: None,
                name_template: "{name}".to_string(),
                addition_suffix: "x".to_string(),
                artist: None,
                content_date: None,
                description: None,
                output_dir: "Example TV".to_string(),
            })
            .unwrap();
        (database, stream, program)
    }

    fn version(stream: i64, program: i64) -> NewVersion {
        NewVersion {
            program_id: program,
            stream_id: stream,
            period: 100.0,
            anchor_time: 10.0,
            seen_at: 5,
            on_air_from: 0.0,
        }
    }

    fn change(stream: i64, program: i64, old: i64) -> NewChange {
        NewChange {
            program_id: program,
            stream_id: stream,
            detected_at: 1,
            old_version_id: old,
            start_time: 50.0,
            delta: 7.0,
            dvr_deadline_at: 9,
        }
    }

    #[test]
    fn the_newest_version_on_air_is_current() {
        let (database, stream, program) = database();
        let first = database.insert_version(&version(stream, program)).unwrap();
        let second = database.insert_version(&version(stream, program)).unwrap();

        assert_eq!(
            database.current_version(program).unwrap().unwrap().id,
            second
        );
        database.supersede_version(second, 90.0, 80.0).unwrap();
        assert_eq!(
            database.current_version(program).unwrap().unwrap().id,
            first
        );
    }

    #[test]
    fn a_program_has_one_open_change() {
        let (database, stream, program) = database();
        let old = database.insert_version(&version(stream, program)).unwrap();
        database
            .insert_change(&change(stream, program, old))
            .unwrap();

        assert!(
            database
                .insert_change(&change(stream, program, old))
                .is_err()
        );
    }

    #[test]
    fn a_later_signal_is_recorded() {
        let (database, stream, program) = database();
        let old = database.insert_version(&version(stream, program)).unwrap();
        let id = database
            .insert_change(&change(stream, program, old))
            .unwrap();

        database.update_change_signal(id, 70.0, 9.0).unwrap();

        let open = database.open_change(program).unwrap().unwrap();
        assert_eq!((open.start_time, open.last_signal_time), (50.0, 70.0));
        assert_eq!(open.last_delta, 9.0);
    }

    #[test]
    fn a_confirmed_change_is_no_longer_open_and_keeps_its_regions() {
        let (database, stream, program) = database();
        let old = database.insert_version(&version(stream, program)).unwrap();
        let new = database.insert_version(&version(stream, program)).unwrap();
        let id = database
            .insert_change(&change(stream, program, old))
            .unwrap();

        database
            .confirm_change(&Confirmation {
                change_id: id,
                new_version_id: new,
                checks: "[]".to_string(),
                boundaries: "[1.5]".to_string(),
                regions: vec![NewRegion {
                    class: "new".to_string(),
                    start: 1.0,
                    end: 2.0,
                    match_version: None,
                    match_start: None,
                }],
            })
            .unwrap();

        assert_eq!(database.open_change(program).unwrap(), None);
        let regions: i64 = database
            .connection()
            .query_row("SELECT COUNT(*) FROM change_regions", [], |row| row.get(0))
            .unwrap();
        assert_eq!(regions, 1);
    }

    #[test]
    fn tracking_overlap_replaces_runs_and_moves_the_watermark() {
        let (database, stream, _) = database();
        let run = |start: f64| StoredRun {
            start,
            end: start + 1.0,
            kind: "lag",
            lag: Some(100.0),
        };
        let state = |tracked_until| AnalysisState {
            tracked_until,
            widened: false,
        };
        database
            .save_tracking(stream, 0.0, &[run(1.0), run(5.0)], state(6.0))
            .unwrap();

        database
            .save_tracking(stream, 4.0, &[run(5.0), run(8.0)], state(9.0))
            .unwrap();

        let count: i64 = database
            .connection()
            .query_row("SELECT COUNT(*) FROM lag_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 3);
        assert_eq!(
            database
                .analysis_state(stream)
                .unwrap()
                .unwrap()
                .tracked_until,
            9.0
        );
    }

    #[test]
    fn a_notification_is_written_once() {
        let (database, _, _) = database();

        database
            .enqueue_notification("change", "c:1", "{}", 0)
            .unwrap();
        database
            .enqueue_notification("change", "c:1", "{}", 1)
            .unwrap();

        let count: i64 = database
            .connection()
            .query_row("SELECT COUNT(*) FROM notifications", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn atomic_work_that_fails_leaves_nothing() {
        let (database, stream, program) = database();

        let result: Result<(), StoreError> = database.atomically(|database| {
            database.insert_version(&version(stream, program))?;
            Err(StoreError::Invalid("stop"))
        });

        assert!(result.is_err());
        assert_eq!(database.current_version(program).unwrap(), None);
    }

    #[test]
    fn a_period_can_be_set_and_read() {
        let (database, stream, _) = database();
        assert_eq!(database.stream_period(stream).unwrap(), None);

        database.set_stream_period(stream, 12.5).unwrap();

        assert_eq!(database.stream_period(stream).unwrap(), Some(12.5));
    }
}

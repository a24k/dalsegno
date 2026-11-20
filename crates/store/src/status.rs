//! What the API shows: read-only views across the tables.

use crate::database::{Database, StoreError};
use crate::streams::StreamState;
use rusqlite::{OptionalExtension, Row, params};

/// A stream as the operator sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamStatus {
    /// Row ID.
    pub id: i64,
    /// Video ID.
    pub video_id: String,
    /// Title.
    pub title: String,
    /// Where it is in its life.
    pub state: StreamState,
    /// Why it is there.
    pub state_reason: Option<String>,
    /// Loop period, once found.
    pub period: Option<f64>,
    /// Next `sq` to follow.
    pub follow_sq: Option<u64>,
    /// When a fragment was last stored.
    pub last_fetch_at: Option<i64>,
    /// Fragments in gaps still to be filled, of every format.
    pub open_fragments: u64,
    /// Fragments in gaps that left the DVR first, which stay holes.
    pub lost_fragments: u64,
    /// The program the stream belongs to, if it has one.
    pub program_id: Option<i64>,
}

/// A change as the operator sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeStatus {
    /// Row ID.
    pub id: i64,
    /// Program.
    pub program_id: i64,
    /// Stream.
    pub stream_id: i64,
    /// `provisional`, `confirmed`, `resolved` or `dismissed`.
    pub state: String,
    /// When it was seen.
    pub detected_at: i64,
    /// When the old version must be exported by.
    pub dvr_deadline_at: Option<i64>,
    /// Stream time of the first signal.
    pub start_time: f64,
    /// JSON array of self-checks.
    pub checks: Option<String>,
    /// JSON array of video boundaries.
    pub boundaries: Option<String>,
}

/// A version as the operator sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct VersionStatus {
    /// Row ID.
    pub id: i64,
    /// The stream that observed it.
    pub stream_id: i64,
    /// Loop period of the audio, in seconds.
    pub period_audio: f64,
    /// Loop period of the video, in seconds, once measured.
    pub period_video: Option<f64>,
    /// Stream time of the start of the held cycle.
    pub anchor_time: f64,
    /// When it was first seen.
    pub first_seen_at: i64,
    /// Stream time it went on air.
    pub on_air_from: f64,
    /// Stream time it was superseded, while it is not current.
    pub on_air_until: Option<f64>,
    /// Stream time of the last cycle start, once superseded.
    pub last_cycle_start: Option<f64>,
}

/// A stored stretch of a comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionStatus {
    /// `existing`, `new`, `revival`, `known`, `deleted` or `no_data`.
    pub class: String,
    /// Start.
    pub start: f64,
    /// End.
    pub end: f64,
    /// Where it starts in the version it matched.
    pub match_start: Option<f64>,
}

const STREAM_COLUMNS: &str = "id, video_id, title, state, state_reason, period, follow_sq, \
     last_fetch_at, \
     (SELECT COALESCE(SUM(sq_last - sq_first + 1), 0) FROM gaps \
      WHERE gaps.stream_id = streams.id AND gaps.status = 'open'), \
     (SELECT COALESCE(SUM(sq_last - sq_first + 1), 0) FROM gaps \
      WHERE gaps.stream_id = streams.id AND gaps.status = 'lost'), \
     program_id";
const VERSION_COLUMNS: &str = "id, stream_id, period_audio, period_video, anchor_time, \
                               first_seen_at, on_air_from_time, on_air_until_time, \
                               last_cycle_start_time";
const CHANGE_COLUMNS: &str = "id, program_id, stream_id, state, detected_at, dvr_deadline_at, \
                              start_time, checks, boundaries";

type RawStream = (StreamStatus, String);

fn read_stream(row: &Row<'_>) -> rusqlite::Result<RawStream> {
    let status = StreamStatus {
        id: row.get(0)?,
        video_id: row.get(1)?,
        title: row.get(2)?,
        state: StreamState::Registered,
        state_reason: row.get(4)?,
        period: row.get(5)?,
        follow_sq: row.get::<_, Option<i64>>(6)?.map(|sq| sq as u64),
        last_fetch_at: row.get(7)?,
        open_fragments: row.get::<_, i64>(8)? as u64,
        lost_fragments: row.get::<_, i64>(9)? as u64,
        program_id: row.get(10)?,
    };
    Ok((status, row.get(3)?))
}

fn checked((status, state): RawStream) -> Result<StreamStatus, StoreError> {
    Ok(StreamStatus {
        state: StreamState::parse(&state)?,
        ..status
    })
}

fn read_change(row: &Row<'_>) -> rusqlite::Result<ChangeStatus> {
    Ok(ChangeStatus {
        id: row.get(0)?,
        program_id: row.get(1)?,
        stream_id: row.get(2)?,
        state: row.get(3)?,
        detected_at: row.get(4)?,
        dvr_deadline_at: row.get(5)?,
        start_time: row.get(6)?,
        checks: row.get(7)?,
        boundaries: row.get(8)?,
    })
}

fn read_version(row: &Row<'_>) -> rusqlite::Result<VersionStatus> {
    Ok(VersionStatus {
        id: row.get(0)?,
        stream_id: row.get(1)?,
        period_audio: row.get(2)?,
        period_video: row.get(3)?,
        anchor_time: row.get(4)?,
        first_seen_at: row.get(5)?,
        on_air_from: row.get(6)?,
        on_air_until: row.get(7)?,
        last_cycle_start: row.get(8)?,
    })
}

impl Database {
    /// Returns a page of streams, oldest first.
    pub fn list_streams(&self, limit: u32, offset: u32) -> Result<Vec<StreamStatus>, StoreError> {
        let sql = format!("SELECT {STREAM_COLUMNS} FROM streams ORDER BY id LIMIT ?1 OFFSET ?2");
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params![limit, offset], read_stream)?;
        rows.map(|row| checked(row?)).collect()
    }

    /// Returns a stream.
    pub fn stream_status(&self, id: i64) -> Result<Option<StreamStatus>, StoreError> {
        let sql = format!("SELECT {STREAM_COLUMNS} FROM streams WHERE id = ?1");
        self.connection
            .query_row(&sql, [id], read_stream)
            .optional()?
            .map(checked)
            .transpose()
    }

    /// Returns a page of changes, newest first.
    pub fn list_changes(&self, limit: u32, offset: u32) -> Result<Vec<ChangeStatus>, StoreError> {
        let sql =
            format!("SELECT {CHANGE_COLUMNS} FROM changes ORDER BY id DESC LIMIT ?1 OFFSET ?2");
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params![limit, offset], read_change)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Returns a page of a program's changes, newest first.
    pub fn list_program_changes(
        &self,
        program_id: i64,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ChangeStatus>, StoreError> {
        let sql = format!(
            "SELECT {CHANGE_COLUMNS} FROM changes WHERE program_id = ?1 \
             ORDER BY id DESC LIMIT ?2 OFFSET ?3"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params![program_id, limit, offset], read_change)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Returns how many changes a program has.
    pub fn count_program_changes(&self, program_id: i64) -> Result<u64, StoreError> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM changes WHERE program_id = ?1",
            [program_id],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    /// Returns a page of a program's versions, newest first.
    pub fn list_versions(
        &self,
        program_id: i64,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<VersionStatus>, StoreError> {
        let sql = format!(
            "SELECT {VERSION_COLUMNS} FROM versions WHERE program_id = ?1 \
             ORDER BY id DESC LIMIT ?2 OFFSET ?3"
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params![program_id, limit, offset], read_version)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Returns how many versions a program has.
    pub fn count_versions(&self, program_id: i64) -> Result<u64, StoreError> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM versions WHERE program_id = ?1",
            [program_id],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    /// Returns a change.
    pub fn change_status(&self, id: i64) -> Result<Option<ChangeStatus>, StoreError> {
        let sql = format!("SELECT {CHANGE_COLUMNS} FROM changes WHERE id = ?1");
        Ok(self
            .connection
            .query_row(&sql, [id], read_change)
            .optional()?)
    }

    /// Returns the comparison of a change, in time order.
    pub fn change_regions(&self, id: i64) -> Result<Vec<RegionStatus>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT class, start_time, end_time, match_start_time FROM change_regions \
             WHERE change_id = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([id], |row| {
            Ok(RegionStatus {
                class: row.get(0)?,
                start: row.get(1)?,
                end: row.get(2)?,
                match_start: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Returns how many rows a list has in all.
    pub fn count_rows(&self, list: Listing) -> Result<u64, StoreError> {
        let sql = format!("SELECT COUNT(*) FROM {}", list.table());
        let count: i64 = self.connection.query_row(&sql, [], |row| row.get(0))?;
        Ok(count as u64)
    }
}

/// A paginated list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing {
    /// Streams.
    Streams,
    /// Changes.
    Changes,
    /// Jobs.
    Jobs,
    /// Programs.
    Programs,
}

impl Listing {
    fn table(self) -> &'static str {
        match self {
            Self::Streams => "streams",
            Self::Changes => "changes",
            Self::Jobs => "jobs",
            Self::Programs => "programs",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::{Confirmation, Dismissal, NewChange, NewRegion, NewVersion};
    use crate::gaps::GapStatus;
    use crate::jobs::{JobClass, NewJob};
    use crate::programs::NewProgram;
    use crate::streams::tests::new_stream;

    fn database() -> (Database, i64) {
        let database = Database::open_in_memory().unwrap();
        let id = database.insert_stream(&new_stream("abcDEF123_-")).unwrap();
        (database, id)
    }

    #[test]
    fn a_stream_shows_its_state_and_reason() {
        let (database, id) = database();
        database
            .set_stream_state(id, StreamState::Error, Some("offline"))
            .unwrap();
        database.set_follow_sq(id, 42).unwrap();

        let status = database.stream_status(id).unwrap().unwrap();

        assert_eq!(status.state, StreamState::Error);
        assert_eq!(status.state_reason.as_deref(), Some("offline"));
        assert_eq!(status.follow_sq, Some(42));
    }

    #[test]
    fn a_stream_counts_its_unfilled_fragments_by_gap_status() {
        let (database, id) = database();
        database.add_gap(id, 140, 10, 19).unwrap();
        database.add_gap(id, 248, 50, 51).unwrap();
        let lost = database.add_gap(id, 140, 100, 104).unwrap();
        database.set_gap_status(lost, GapStatus::Lost).unwrap();
        let filled = database.add_gap(id, 140, 200, 299).unwrap();
        database.set_gap_status(filled, GapStatus::Filled).unwrap();

        let status = database.stream_status(id).unwrap().unwrap();
        let listed = database.list_streams(10, 0).unwrap();

        assert_eq!((status.open_fragments, status.lost_fragments), (12, 5));
        assert_eq!(listed[0], status);
    }

    #[test]
    fn a_stream_without_gaps_counts_none() {
        let (database, id) = database();

        let status = database.stream_status(id).unwrap().unwrap();

        assert_eq!((status.open_fragments, status.lost_fragments), (0, 0));
    }

    #[test]
    fn an_unknown_stream_is_none() {
        assert_eq!(database().0.stream_status(99).unwrap(), None);
    }

    #[test]
    fn streams_are_paged_and_counted() {
        let (database, first) = database();
        let second = database.insert_stream(&new_stream("zzzZZZ999_-")).unwrap();

        let page = database.list_streams(1, 1).unwrap();

        assert_eq!(page[0].id, second);
        assert_ne!(first, second);
        assert_eq!(database.count_rows(Listing::Streams).unwrap(), 2);
    }

    #[test]
    fn no_changes_list_as_nothing() {
        let (database, _) = database();

        assert_eq!(database.list_changes(10, 0).unwrap(), []);
        assert_eq!(database.change_status(1).unwrap(), None);
        assert_eq!(database.change_regions(1).unwrap(), []);
    }

    fn program(database: &Database) -> i64 {
        database
            .insert_program(&NewProgram {
                short_name: "Example TV".to_string(),
                channel_id: None,
                name_template: "{name} {datetime}{suffix} ({year})".to_string(),
                addition_suffix: "addition".to_string(),
                artist: None,
                content_date: None,
                description: None,
                output_dir: "Example TV".to_string(),
            })
            .unwrap()
    }

    fn version(database: &Database, program_id: i64, stream_id: i64, anchor_time: f64) -> i64 {
        database
            .insert_version(&NewVersion {
                program_id,
                stream_id,
                period: 100.0,
                anchor_time,
                seen_at: 7,
                on_air_from: anchor_time,
            })
            .unwrap()
    }

    fn change(database: &Database, program_id: i64, stream_id: i64, old_version_id: i64) -> i64 {
        database
            .insert_change(&NewChange {
                program_id,
                stream_id,
                detected_at: 2_000,
                old_version_id,
                start_time: 30.0,
                delta: 1.5,
                dvr_deadline_at: 9_000,
            })
            .unwrap()
    }

    fn dismissal(change_id: i64) -> Dismissal {
        Dismissal {
            change_id,
            checks: "[]".to_string(),
            regions: Vec::new(),
        }
    }

    #[test]
    fn a_stream_shows_the_program_it_belongs_to() {
        let (database, id) = database();
        let program_id = program(&database);
        database.link_stream_to_program(id, program_id).unwrap();

        let status = database.stream_status(id).unwrap().unwrap();

        assert_eq!(status.program_id, Some(program_id));
    }

    #[test]
    fn a_stream_without_a_program_shows_none() {
        let (database, id) = database();

        let status = database.stream_status(id).unwrap().unwrap();

        assert_eq!(status.program_id, None);
    }

    #[test]
    fn a_stream_shows_its_registered_values_and_last_fetch() {
        let (database, id) = database();
        database.set_last_fetch(id, 5_000).unwrap();
        database.set_stream_period(id, 1_800.0).unwrap();

        let status = database.stream_status(id).unwrap().unwrap();

        assert_eq!(status.video_id, "abcDEF123_-");
        assert_eq!(status.title, "Example TV -An Archive-");
        assert_eq!(status.state, StreamState::Registered);
        assert_eq!(status.last_fetch_at, Some(5_000));
        assert_eq!(status.period, Some(1_800.0));
        assert_eq!(status.follow_sq, None);
    }

    #[test]
    fn streams_list_oldest_first() {
        let (database, first) = database();
        let second = database.insert_stream(&new_stream("zzzZZZ999_-")).unwrap();

        let listed = database.list_streams(10, 0).unwrap();

        let ids: Vec<i64> = listed.iter().map(|stream| stream.id).collect();
        assert_eq!(ids, [first, second]);
    }

    #[test]
    fn a_change_shows_what_was_stored_when_it_opened() {
        let (database, stream_id) = database();
        let program_id = program(&database);
        let old = version(&database, program_id, stream_id, 0.0);
        let id = change(&database, program_id, stream_id, old);

        let status = database.change_status(id).unwrap().unwrap();

        assert_eq!(
            status,
            ChangeStatus {
                id,
                program_id,
                stream_id,
                state: "provisional".to_string(),
                detected_at: 2_000,
                dvr_deadline_at: Some(9_000),
                start_time: 30.0,
                checks: None,
                boundaries: None,
            }
        );
    }

    #[test]
    fn a_confirmed_change_shows_its_checks_and_boundaries() {
        let (database, stream_id) = database();
        let program_id = program(&database);
        let old = version(&database, program_id, stream_id, 0.0);
        let new = version(&database, program_id, stream_id, 500.0);
        let id = change(&database, program_id, stream_id, old);
        database
            .confirm_change(&Confirmation {
                change_id: id,
                new_version_id: new,
                checks: "[\"a\"]".to_string(),
                boundaries: "[1.0]".to_string(),
                regions: Vec::new(),
            })
            .unwrap();

        let status = database.change_status(id).unwrap().unwrap();

        assert_eq!(status.state, "confirmed");
        assert_eq!(status.checks.as_deref(), Some("[\"a\"]"));
        assert_eq!(status.boundaries.as_deref(), Some("[1.0]"));
    }

    #[test]
    fn changes_list_newest_first_and_page() {
        let (database, stream_id) = database();
        let program_id = program(&database);
        let old = version(&database, program_id, stream_id, 0.0);
        let first = change(&database, program_id, stream_id, old);
        database.dismiss_change(&dismissal(first)).unwrap();
        let second = change(&database, program_id, stream_id, old);
        database.dismiss_change(&dismissal(second)).unwrap();
        let third = change(&database, program_id, stream_id, old);

        let all = database.list_changes(10, 0).unwrap();
        let page = database.list_changes(1, 1).unwrap();

        let ids: Vec<i64> = all.iter().map(|change| change.id).collect();
        assert_eq!(ids, [third, second, first]);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, second);
    }

    #[test]
    fn a_program_lists_only_its_own_changes_newest_first_and_counts_them() {
        let (database, stream_id) = database();
        let program_id = program(&database);
        let other_id = program(&database);
        let old = version(&database, program_id, stream_id, 0.0);
        let other_old = version(&database, other_id, stream_id, 0.0);
        let first = change(&database, program_id, stream_id, old);
        database.dismiss_change(&dismissal(first)).unwrap();
        let second = change(&database, program_id, stream_id, old);
        change(&database, other_id, stream_id, other_old);

        let listed = database.list_program_changes(program_id, 10, 0).unwrap();
        let page = database.list_program_changes(program_id, 1, 1).unwrap();

        let ids: Vec<i64> = listed.iter().map(|change| change.id).collect();
        assert_eq!(ids, [second, first]);
        assert_eq!(page[0].id, first);
        assert_eq!(database.count_program_changes(program_id).unwrap(), 2);
        assert_eq!(database.count_program_changes(other_id).unwrap(), 1);
        assert_eq!(database.count_program_changes(99).unwrap(), 0);
    }

    #[test]
    fn a_program_lists_its_versions_newest_first_and_counts_them() {
        let (database, stream_id) = database();
        let program_id = program(&database);
        let other_id = program(&database);
        let first = version(&database, program_id, stream_id, 0.0);
        let second = version(&database, program_id, stream_id, 500.0);
        version(&database, other_id, stream_id, 0.0);

        let listed = database.list_versions(program_id, 10, 0).unwrap();
        let page = database.list_versions(program_id, 1, 1).unwrap();

        let ids: Vec<i64> = listed.iter().map(|version| version.id).collect();
        assert_eq!(ids, [second, first]);
        assert_eq!(page[0].id, first);
        assert_eq!(database.count_versions(program_id).unwrap(), 2);
        assert_eq!(database.count_versions(99).unwrap(), 0);
    }

    #[test]
    fn a_version_shows_what_was_stored_when_it_was_observed() {
        let (database, stream_id) = database();
        let program_id = program(&database);
        let id = version(&database, program_id, stream_id, 500.0);

        let listed = database.list_versions(program_id, 10, 0).unwrap();

        assert_eq!(
            listed,
            [VersionStatus {
                id,
                stream_id,
                period_audio: 100.0,
                period_video: None,
                anchor_time: 500.0,
                first_seen_at: 7,
                on_air_from: 500.0,
                on_air_until: None,
                last_cycle_start: None,
            }]
        );
    }

    #[test]
    fn an_unknown_change_is_none() {
        let (database, _) = database();

        assert_eq!(database.change_status(99).unwrap(), None);
    }

    #[test]
    fn a_change_keeps_its_regions_in_the_order_stored() {
        let (database, stream_id) = database();
        let program_id = program(&database);
        let old = version(&database, program_id, stream_id, 0.0);
        let id = change(&database, program_id, stream_id, old);
        let region = |class: &str, start: f64, end: f64, match_start: Option<f64>| NewRegion {
            class: class.to_string(),
            start,
            end,
            match_version: match_start.map(|_| old),
            match_start,
        };
        database
            .dismiss_change(&Dismissal {
                change_id: id,
                checks: "[]".to_string(),
                regions: vec![
                    region("existing", 0.0, 10.0, Some(5.0)),
                    region("new", 10.0, 20.0, None),
                ],
            })
            .unwrap();

        let regions = database.change_regions(id).unwrap();

        assert_eq!(
            regions,
            [
                RegionStatus {
                    class: "existing".to_string(),
                    start: 0.0,
                    end: 10.0,
                    match_start: Some(5.0),
                },
                RegionStatus {
                    class: "new".to_string(),
                    start: 10.0,
                    end: 20.0,
                    match_start: None,
                },
            ]
        );
    }

    #[test]
    fn rows_are_counted_for_each_listing() {
        let (database, stream_id) = database();
        let program_id = program(&database);
        let old = version(&database, program_id, stream_id, 0.0);
        change(&database, program_id, stream_id, old);
        for target in ["{\"a\":1}", "{\"a\":2}", "{\"a\":3}"] {
            database
                .enqueue_job(
                    &NewJob {
                        kind: "features".to_string(),
                        class: JobClass::Cpu,
                        target: target.to_string(),
                    },
                    1,
                )
                .unwrap();
        }

        assert_eq!(database.count_rows(Listing::Streams).unwrap(), 1);
        assert_eq!(database.count_rows(Listing::Changes).unwrap(), 1);
        assert_eq!(database.count_rows(Listing::Jobs).unwrap(), 3);
        assert_eq!(database.count_rows(Listing::Programs).unwrap(), 1);
    }

    #[test]
    fn an_empty_database_counts_no_rows() {
        let database = Database::open_in_memory().unwrap();

        assert_eq!(database.count_rows(Listing::Streams).unwrap(), 0);
        assert_eq!(database.count_rows(Listing::Changes).unwrap(), 0);
        assert_eq!(database.count_rows(Listing::Jobs).unwrap(), 0);
        assert_eq!(database.count_rows(Listing::Programs).unwrap(), 0);
    }
}

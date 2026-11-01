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

const STREAM_COLUMNS: &str =
    "id, video_id, title, state, state_reason, period, follow_sq, last_fetch_at";
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
}

impl Listing {
    fn table(self) -> &'static str {
        match self {
            Self::Streams => "streams",
            Self::Changes => "changes",
            Self::Jobs => "jobs",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
}

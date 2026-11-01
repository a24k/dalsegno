//! Registered streams.

use crate::database::{Database, StoreError};
use rusqlite::{OptionalExtension, Row, params};

const STREAM_COLUMNS: &str = "id, video_id, title, channel_id, channel_name, thumbnail_url, \
                              video_itag, audio_itag, fragment_len, registered_at";

/// Where a stream is in its life (FR-S4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamState {
    /// Known, and not started.
    Registered,
    /// Started, and the head has not been reached yet.
    Backfilling,
    /// Following the head.
    Recording,
    /// Stopped by a person.
    Paused,
    /// Stopped by a failure. The reason says which.
    Error,
    /// The stream has finished.
    Ended,
}

impl StreamState {
    /// Returns the stored name, which the API also shows.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Registered => "registered",
            Self::Backfilling => "backfilling",
            Self::Recording => "recording",
            Self::Paused => "paused",
            Self::Error => "error",
            Self::Ended => "ended",
        }
    }

    pub(crate) fn parse(text: &str) -> Result<Self, StoreError> {
        match text {
            "registered" => Ok(Self::Registered),
            "backfilling" => Ok(Self::Backfilling),
            "recording" => Ok(Self::Recording),
            "paused" => Ok(Self::Paused),
            "error" => Ok(Self::Error),
            "ended" => Ok(Self::Ended),
            _ => Err(StoreError::Invalid("stream state")),
        }
    }
}

/// What registration knows about a stream.
#[derive(Debug, Clone, PartialEq)]
pub struct NewStream {
    /// Video ID.
    pub video_id: String,
    /// Stream title.
    pub title: String,
    /// Channel ID.
    pub channel_id: Option<String>,
    /// Channel name.
    pub channel_name: Option<String>,
    /// Thumbnail reference.
    pub thumbnail_url: Option<String>,
    /// Chosen video format.
    pub video_itag: Option<u32>,
    /// Chosen audio format.
    pub audio_itag: u32,
    /// Measured fragment length, in seconds.
    pub fragment_len: f64,
    /// Registration time, in epoch milliseconds.
    pub registered_at: i64,
}

/// A stored stream.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamRow {
    /// Row ID, which also names the stream's fragment directory.
    pub id: i64,
    /// The registered values.
    pub stream: NewStream,
}

fn stream_from_row(row: &Row<'_>) -> rusqlite::Result<StreamRow> {
    Ok(StreamRow {
        id: row.get(0)?,
        stream: NewStream {
            video_id: row.get(1)?,
            title: row.get(2)?,
            channel_id: row.get(3)?,
            channel_name: row.get(4)?,
            thumbnail_url: row.get(5)?,
            video_itag: row.get(6)?,
            audio_itag: row.get(7)?,
            fragment_len: row.get(8)?,
            registered_at: row.get(9)?,
        },
    })
}

impl Database {
    /// Stores a new stream in the `registered` state, and returns its row ID.
    pub fn insert_stream(&self, stream: &NewStream) -> Result<i64, StoreError> {
        self.connection.execute(
            "INSERT INTO streams (video_id, title, channel_id, channel_name, thumbnail_url, \
             state, video_itag, audio_itag, fragment_len, registered_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, 'registered', ?6, ?7, ?8, ?9)",
            params![
                stream.video_id,
                stream.title,
                stream.channel_id,
                stream.channel_name,
                stream.thumbnail_url,
                stream.video_itag,
                stream.audio_itag,
                stream.fragment_len,
                stream.registered_at,
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Finds a stream by its video ID.
    pub fn stream_by_video_id(&self, video_id: &str) -> Result<Option<StreamRow>, StoreError> {
        let sql = format!("SELECT {STREAM_COLUMNS} FROM streams WHERE video_id = ?1");
        Ok(self
            .connection
            .query_row(&sql, [video_id], stream_from_row)
            .optional()?)
    }

    /// Returns the stream with this row ID.
    pub fn stream(&self, stream_id: i64) -> Result<Option<StreamRow>, StoreError> {
        let sql = format!("SELECT {STREAM_COLUMNS} FROM streams WHERE id = ?1");
        Ok(self
            .connection
            .query_row(&sql, [stream_id], stream_from_row)
            .optional()?)
    }

    /// Returns the streams in any of these states, oldest first.
    pub fn streams_in_states(&self, states: &[StreamState]) -> Result<Vec<StreamRow>, StoreError> {
        let wanted: Vec<String> = states
            .iter()
            .map(|state| format!("'{}'", state.as_str()))
            .collect();
        let sql = format!(
            "SELECT {STREAM_COLUMNS} FROM streams WHERE state IN ({}) ORDER BY id",
            wanted.join(", ")
        );
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map([], stream_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Returns the state of a stream and why it is in it.
    pub fn stream_state(
        &self,
        stream_id: i64,
    ) -> Result<Option<(StreamState, Option<String>)>, StoreError> {
        let row: Option<(String, Option<String>)> = self
            .connection
            .query_row(
                "SELECT state, state_reason FROM streams WHERE id = ?1",
                [stream_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(state, reason)| Ok((StreamState::parse(&state)?, reason)))
            .transpose()
    }

    /// Moves a stream to a state, with the reason to show (FR-S4).
    pub fn set_stream_state(
        &self,
        stream_id: i64,
        state: StreamState,
        reason: Option<&str>,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE streams SET state = ?2, state_reason = ?3 WHERE id = ?1",
            params![stream_id, state.as_str(), reason],
        )?;
        Ok(())
    }

    /// Returns the next `sq` to follow, if following has ever started.
    pub fn follow_sq(&self, stream_id: i64) -> Result<Option<u64>, StoreError> {
        let sq: Option<Option<i64>> = self
            .connection
            .query_row(
                "SELECT follow_sq FROM streams WHERE id = ?1",
                [stream_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(sq.flatten().map(|sq| sq as u64))
    }

    /// Sets the next `sq` to follow.
    pub fn set_follow_sq(&self, stream_id: i64, sq: u64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE streams SET follow_sq = ?2 WHERE id = ?1",
            params![stream_id, sq as i64],
        )?;
        Ok(())
    }

    /// Moves following past a stored head fragment. The first one ends a backfilling state,
    /// and a stream stopped meanwhile stays stopped.
    pub fn advance_follow(&self, stream_id: i64, sq: u64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE streams SET follow_sq = ?2, \
             state = CASE state WHEN 'backfilling' THEN 'recording' ELSE state END \
             WHERE id = ?1",
            params![stream_id, sq as i64],
        )?;
        Ok(())
    }

    /// Records the time of the latest stored fragment.
    pub fn set_last_fetch(&self, stream_id: i64, at: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE streams SET last_fetch_at = ?2 WHERE id = ?1",
            params![stream_id, at],
        )?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn new_stream(video_id: &str) -> NewStream {
        NewStream {
            video_id: video_id.to_string(),
            title: "Example TV -An Archive-".to_string(),
            channel_id: Some("UCexample".to_string()),
            channel_name: None,
            thumbnail_url: None,
            video_itag: Some(248),
            audio_itag: 140,
            fragment_len: 5.0,
            registered_at: 1_000,
        }
    }

    #[test]
    fn stored_stream_can_be_found_by_video_id() {
        let database = Database::open_in_memory().unwrap();
        let stream = new_stream("abcDEF123_-");

        let id = database.insert_stream(&stream).unwrap();

        let row = database.stream_by_video_id("abcDEF123_-").unwrap().unwrap();
        assert_eq!(row, StreamRow { id, stream });
    }

    #[test]
    fn unknown_video_id_is_absent() {
        let database = Database::open_in_memory().unwrap();

        assert_eq!(database.stream_by_video_id("nope").unwrap(), None);
    }

    #[test]
    fn a_video_id_registers_once() {
        let database = Database::open_in_memory().unwrap();
        database.insert_stream(&new_stream("abcDEF123_-")).unwrap();

        assert!(database.insert_stream(&new_stream("abcDEF123_-")).is_err());
    }

    #[test]
    fn last_fetch_is_recorded() {
        let database = Database::open_in_memory().unwrap();
        let id = database.insert_stream(&new_stream("abcDEF123_-")).unwrap();

        database.set_last_fetch(id, 5_000).unwrap();

        let at: i64 = database
            .connection()
            .query_row("SELECT last_fetch_at FROM streams", [], |row| row.get(0))
            .unwrap();
        assert_eq!(at, 5_000);
    }
}

//! Registered streams.

use crate::database::{Database, StoreError};
use rusqlite::{OptionalExtension, Row, params};

const STREAM_COLUMNS: &str = "id, video_id, title, channel_id, channel_name, thumbnail_url, \
                              video_itag, audio_itag, fragment_len, registered_at";

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

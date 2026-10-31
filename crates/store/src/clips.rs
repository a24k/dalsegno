//! Cuts, with everything needed to make them again (FR-E12).

use crate::database::{Database, StoreError};
use rusqlite::{Connection, OptionalExtension, Row, params};

const CLIP_COLUMNS: &str = "id, name, kind, stream_id, video_itag, audio_itag, sq_first, sq_last,
                            duration, metadata, poster_time, output_path, plex_status";

/// What a clip holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipKind {
    /// A whole cycle.
    Cycle,
    /// Only an addition.
    Addition,
}

impl ClipKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cycle => "cycle",
            Self::Addition => "addition",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "cycle" => Some(Self::Cycle),
            "addition" => Some(Self::Addition),
            _ => None,
        }
    }
}

/// Where a clip stands with Plex.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlexStatus {
    /// No rescan was asked for.
    None,
    /// A rescan was asked for.
    Requested,
    /// Plex lists the file.
    Confirmed,
    /// Plex did not list the file in time.
    Failed,
}

impl PlexStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Requested => "requested",
            Self::Confirmed => "confirmed",
            Self::Failed => "failed",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "none" => Some(Self::None),
            "requested" => Some(Self::Requested),
            "confirmed" => Some(Self::Confirmed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// A cut to record.
#[derive(Debug, Clone, PartialEq)]
pub struct NewClip {
    /// Output name, which is unique.
    pub name: String,
    /// What it holds.
    pub kind: ClipKind,
    /// Stream.
    pub stream_id: i64,
    /// Video format.
    pub video_itag: u32,
    /// Audio format.
    pub audio_itag: u32,
    /// First `sq`, whose start is the cut's keyframe.
    pub sq_first: u64,
    /// Last `sq`.
    pub sq_last: u64,
    /// Length, in seconds.
    pub duration: f64,
    /// Tags as written to the mp4, as JSON.
    pub metadata: String,
    /// Stream time of the poster frame, for an addition.
    pub poster_time: Option<f64>,
}

/// A stored cut.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipRow {
    /// Row ID.
    pub id: i64,
    /// The recorded values.
    pub clip: NewClip,
    /// Finished folder, once it exists.
    pub output_path: Option<String>,
    /// Where it stands with Plex.
    pub plex_status: PlexStatus,
}

fn invalid(column: usize, text: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidColumnType(column, text.to_string(), rusqlite::types::Type::Text)
}

fn clip_from_row(row: &Row<'_>) -> rusqlite::Result<ClipRow> {
    let kind: String = row.get(2)?;
    let status: String = row.get(12)?;
    Ok(ClipRow {
        id: row.get(0)?,
        clip: NewClip {
            name: row.get(1)?,
            kind: ClipKind::parse(&kind).ok_or_else(|| invalid(2, &kind))?,
            stream_id: row.get(3)?,
            video_itag: row.get(4)?,
            audio_itag: row.get(5)?,
            sq_first: row.get::<_, i64>(6)? as u64,
            sq_last: row.get::<_, i64>(7)? as u64,
            duration: row.get(8)?,
            metadata: row.get(9)?,
            poster_time: row.get(10)?,
        },
        output_path: row.get(11)?,
        plex_status: PlexStatus::parse(&status).ok_or_else(|| invalid(12, &status))?,
    })
}

fn insert_clip_row(connection: &Connection, clip: &NewClip) -> Result<i64, StoreError> {
    connection.execute(
        "INSERT INTO clips (name, kind, stream_id, video_itag, audio_itag, sq_first, sq_last,
                            duration, metadata, poster_time)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            clip.name,
            clip.kind.as_str(),
            clip.stream_id,
            clip.video_itag,
            clip.audio_itag,
            clip.sq_first as i64,
            clip.sq_last as i64,
            clip.duration,
            clip.metadata,
            clip.poster_time,
        ],
    )?;
    Ok(connection.last_insert_rowid())
}

impl Database {
    /// Records a cut, and returns its row ID.
    pub fn insert_clip(&self, clip: &NewClip) -> Result<i64, StoreError> {
        insert_clip_row(&self.connection, clip)
    }

    /// Records a cut with holds on both of its tracks, all or nothing, so that no clip exists
    /// whose fragments a sweep could take.
    pub fn insert_held_clip(&mut self, clip: &NewClip) -> Result<i64, StoreError> {
        self.transaction(|transaction| {
            let id = insert_clip_row(transaction, clip)?;
            for itag in [clip.video_itag, clip.audio_itag] {
                transaction.execute(
                    "INSERT INTO retention_holds (stream_id, itag, sq_first, sq_last, clip_id)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        clip.stream_id,
                        itag,
                        clip.sq_first as i64,
                        clip.sq_last as i64,
                        id
                    ],
                )?;
            }
            Ok(id)
        })
    }

    /// Returns a clip by its row ID.
    pub fn clip(&self, id: i64) -> Result<Option<ClipRow>, StoreError> {
        let row = self
            .connection
            .query_row(
                &format!("SELECT {CLIP_COLUMNS} FROM clips WHERE id = ?1"),
                params![id],
                clip_from_row,
            )
            .optional()?;
        Ok(row)
    }

    /// Returns a clip by its name.
    pub fn clip_by_name(&self, name: &str) -> Result<Option<ClipRow>, StoreError> {
        let row = self
            .connection
            .query_row(
                &format!("SELECT {CLIP_COLUMNS} FROM clips WHERE name = ?1"),
                params![name],
                clip_from_row,
            )
            .optional()?;
        Ok(row)
    }

    /// Sets the last `sq` once the cut's fragments are known.
    pub fn set_clip_last_sq(&self, id: i64, sq_last: u64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE clips SET sq_last = ?2 WHERE id = ?1",
            params![id, sq_last as i64],
        )?;
        Ok(())
    }

    /// Records where the finished folder is.
    pub fn set_clip_output(&self, id: i64, path: &str) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE clips SET output_path = ?2 WHERE id = ?1",
            params![id, path],
        )?;
        Ok(())
    }

    /// Records where a clip stands with Plex.
    pub fn set_clip_plex_status(&self, id: i64, status: PlexStatus) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE clips SET plex_status = ?2 WHERE id = ?1",
            params![id, status.as_str()],
        )?;
        Ok(())
    }

    /// Lets the holds of a clip lapse at `at`, once the clip no longer needs its fragments.
    pub fn expire_clip_holds(&self, id: i64, at: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE retention_holds SET expires_at = ?2 WHERE clip_id = ?1",
            params![id, at],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retention::{HoldOwner, NewHold};
    use crate::streams::NewStream;

    fn database() -> Database {
        let database = Database::open_in_memory().unwrap();
        database
            .insert_stream(&NewStream {
                video_id: "AAAAAAAAAAA".to_string(),
                title: "Example".to_string(),
                channel_id: None,
                channel_name: None,
                thumbnail_url: None,
                video_itag: Some(248),
                audio_itag: 140,
                fragment_len: 5.0,
                registered_at: 0,
            })
            .unwrap();
        database
    }

    fn clip(name: &str) -> NewClip {
        NewClip {
            name: name.to_string(),
            kind: ClipKind::Addition,
            stream_id: 1,
            video_itag: 248,
            audio_itag: 140,
            sq_first: 10,
            sq_last: 20,
            duration: 50.0,
            metadata: "{}".to_string(),
            poster_time: Some(90.0),
        }
    }

    #[test]
    fn a_clip_reads_back_as_stored() {
        let database = database();

        let id = database.insert_clip(&clip("Example")).unwrap();

        let found = database.clip(id).unwrap().unwrap();
        assert_eq!(found.clip, clip("Example"));
        assert_eq!(found.plex_status, PlexStatus::None);
        assert_eq!(found.output_path, None);
    }

    #[test]
    fn a_clip_and_its_holds_arrive_together() {
        let mut database = database();

        let id = database.insert_held_clip(&clip("Example")).unwrap();

        let holds: i64 = database
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM retention_holds WHERE clip_id = ?1",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(holds, 2);
    }

    #[test]
    fn a_failed_clip_leaves_no_holds() {
        let mut database = database();
        database.insert_held_clip(&clip("Example")).unwrap();

        assert!(database.insert_held_clip(&clip("Example")).is_err());

        let holds: i64 = database
            .connection()
            .query_row("SELECT COUNT(*) FROM retention_holds", [], |row| row.get(0))
            .unwrap();
        assert_eq!(holds, 2);
    }

    #[test]
    fn names_are_unique() {
        let database = database();
        database.insert_clip(&clip("Example")).unwrap();

        assert!(database.insert_clip(&clip("Example")).is_err());
    }

    #[test]
    fn a_clip_is_found_by_name() {
        let database = database();
        let id = database.insert_clip(&clip("Example")).unwrap();

        assert_eq!(database.clip_by_name("Example").unwrap().unwrap().id, id);
        assert_eq!(database.clip_by_name("Other").unwrap(), None);
    }

    #[test]
    fn progress_is_recorded() {
        let database = database();
        let id = database.insert_clip(&clip("Example")).unwrap();

        database.set_clip_last_sq(id, 19).unwrap();
        database.set_clip_output(id, "/library/Example").unwrap();
        database
            .set_clip_plex_status(id, PlexStatus::Confirmed)
            .unwrap();

        let found = database.clip(id).unwrap().unwrap();
        assert_eq!(found.clip.sq_last, 19);
        assert_eq!(found.output_path.as_deref(), Some("/library/Example"));
        assert_eq!(found.plex_status, PlexStatus::Confirmed);
    }

    #[test]
    fn holds_of_a_clip_can_lapse() {
        let database = database();
        let id = database.insert_clip(&clip("Example")).unwrap();
        let hold = database
            .insert_hold(&NewHold {
                stream_id: 1,
                itag: 248,
                sq_first: 10,
                sq_last: 20,
                owner: HoldOwner::Clip(id),
            })
            .unwrap();

        database.expire_clip_holds(id, 1_000).unwrap();

        let expires: Option<i64> = database
            .connection()
            .query_row(
                "SELECT expires_at FROM retention_holds WHERE id = ?1",
                [hold],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(expires, Some(1_000));
    }

    #[test]
    fn all_kinds_and_statuses_read_back() {
        for kind in [ClipKind::Cycle, ClipKind::Addition] {
            assert_eq!(ClipKind::parse(kind.as_str()), Some(kind));
        }
        for status in [
            PlexStatus::None,
            PlexStatus::Requested,
            PlexStatus::Confirmed,
            PlexStatus::Failed,
        ] {
            assert_eq!(PlexStatus::parse(status.as_str()), Some(status));
        }
    }
}

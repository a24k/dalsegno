//! Programs, which give a stream its naming and where its output goes.

use crate::database::{Database, StoreError};
use rusqlite::{OptionalExtension, Row, params};

const PROGRAM_COLUMNS: &str = "id, short_name, channel_id, name_template, addition_suffix, artist,
                               content_date, description, output_dir";

/// What a program is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProgram {
    /// Short name, which starts each output name.
    pub short_name: String,
    /// Channel ID.
    pub channel_id: Option<String>,
    /// Template for output names.
    pub name_template: String,
    /// Suffix of addition names.
    pub addition_suffix: String,
    /// The `artist` tag.
    pub artist: Option<String>,
    /// The `date` tag, shared by everything from this stream.
    pub content_date: Option<String>,
    /// The `description` tag of a cycle.
    pub description: Option<String>,
    /// Folder inside the library that receives output.
    pub output_dir: String,
}

/// A stored program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramRow {
    /// Row ID.
    pub id: i64,
    /// The stored values.
    pub program: NewProgram,
}

fn program_from_row(row: &Row<'_>) -> rusqlite::Result<ProgramRow> {
    Ok(ProgramRow {
        id: row.get(0)?,
        program: NewProgram {
            short_name: row.get(1)?,
            channel_id: row.get(2)?,
            name_template: row.get(3)?,
            addition_suffix: row.get(4)?,
            artist: row.get(5)?,
            content_date: row.get(6)?,
            description: row.get(7)?,
            output_dir: row.get(8)?,
        },
    })
}

impl Database {
    /// Stores a program, and returns its row ID.
    pub fn insert_program(&self, program: &NewProgram) -> Result<i64, StoreError> {
        self.connection.execute(
            "INSERT INTO programs (short_name, channel_id, name_template, addition_suffix, artist,
                                   content_date, description, output_dir)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                program.short_name,
                program.channel_id,
                program.name_template,
                program.addition_suffix,
                program.artist,
                program.content_date,
                program.description,
                program.output_dir,
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Changes a program, which keeps its versions.
    pub fn update_program(&self, id: i64, program: &NewProgram) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE programs SET short_name = ?2, channel_id = ?3, name_template = ?4,
                                 addition_suffix = ?5, artist = ?6, content_date = ?7,
                                 description = ?8, output_dir = ?9
             WHERE id = ?1",
            params![
                id,
                program.short_name,
                program.channel_id,
                program.name_template,
                program.addition_suffix,
                program.artist,
                program.content_date,
                program.description,
                program.output_dir,
            ],
        )?;
        Ok(())
    }

    /// Makes a stream part of a program.
    pub fn link_stream_to_program(
        &self,
        stream_id: i64,
        program_id: i64,
    ) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE streams SET program_id = ?2 WHERE id = ?1",
            params![stream_id, program_id],
        )?;
        Ok(())
    }

    /// Returns the program of a stream, if it has one.
    pub fn program_of_stream(&self, stream_id: i64) -> Result<Option<ProgramRow>, StoreError> {
        let row = self
            .connection
            .query_row(
                &format!(
                    "SELECT {PROGRAM_COLUMNS} FROM programs
                     WHERE id = (SELECT program_id FROM streams WHERE id = ?1)"
                ),
                params![stream_id],
                program_from_row,
            )
            .optional()?;
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streams::NewStream;

    fn program() -> NewProgram {
        NewProgram {
            short_name: "Example TV".to_string(),
            channel_id: None,
            name_template: "{name} {datetime}{suffix} ({year})".to_string(),
            addition_suffix: "追加分".to_string(),
            artist: Some("Example".to_string()),
            content_date: Some("2026-01-01".to_string()),
            description: None,
            output_dir: "Example TV".to_string(),
        }
    }

    fn stream(database: &Database) -> i64 {
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
            .unwrap()
    }

    #[test]
    fn a_linked_program_comes_back_with_its_stream() {
        let database = Database::open_in_memory().unwrap();
        let stream_id = stream(&database);
        let id = database.insert_program(&program()).unwrap();

        database.link_stream_to_program(stream_id, id).unwrap();

        let found = database.program_of_stream(stream_id).unwrap().unwrap();
        assert_eq!(found.program, program());
    }

    #[test]
    fn a_stream_starts_with_no_program() {
        let database = Database::open_in_memory().unwrap();
        let stream_id = stream(&database);

        assert_eq!(database.program_of_stream(stream_id).unwrap(), None);
    }
}

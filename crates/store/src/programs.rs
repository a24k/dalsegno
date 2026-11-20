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

    /// Returns a program.
    pub fn program(&self, id: i64) -> Result<Option<ProgramRow>, StoreError> {
        let sql = format!("SELECT {PROGRAM_COLUMNS} FROM programs WHERE id = ?1");
        Ok(self
            .connection
            .query_row(&sql, [id], program_from_row)
            .optional()?)
    }

    /// Returns a page of programs, oldest first.
    pub fn list_programs(&self, limit: u32, offset: u32) -> Result<Vec<ProgramRow>, StoreError> {
        let sql = format!("SELECT {PROGRAM_COLUMNS} FROM programs ORDER BY id LIMIT ?1 OFFSET ?2");
        let mut statement = self.connection.prepare(&sql)?;
        let rows = statement.query_map(params![limit, offset], program_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Returns the IDs of the streams linked to a program, ascending.
    pub fn program_stream_ids(&self, program_id: i64) -> Result<Vec<i64>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT id FROM streams WHERE program_id = ?1 ORDER BY id")?;
        let ids = statement.query_map([program_id], |row| row.get(0))?;
        Ok(ids.collect::<Result<_, _>>()?)
    }

    /// Returns the IDs of the streams that observed versions of a program, ascending.
    pub fn version_streams(&self, program_id: i64) -> Result<Vec<i64>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT DISTINCT stream_id FROM versions WHERE program_id = ?1 ORDER BY stream_id",
        )?;
        let ids = statement.query_map([program_id], |row| row.get(0))?;
        Ok(ids.collect::<Result<_, _>>()?)
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
    use crate::changes::NewVersion;
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

    fn stream_values() -> NewStream {
        NewStream {
            video_id: "AAAAAAAAAAA".to_string(),
            title: "Example".to_string(),
            channel_id: None,
            channel_name: None,
            thumbnail_url: None,
            video_itag: Some(248),
            audio_itag: 140,
            fragment_len: 5.0,
            registered_at: 0,
        }
    }

    fn stream(database: &Database) -> i64 {
        database.insert_stream(&stream_values()).unwrap()
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

    #[test]
    fn programs_are_found_listed_and_paged() {
        let database = Database::open_in_memory().unwrap();
        let first = database.insert_program(&program()).unwrap();
        let second = database.insert_program(&program()).unwrap();

        let page = database.list_programs(1, 1).unwrap();

        assert_eq!(page[0].id, second);
        assert_eq!(database.program(first).unwrap().unwrap().program, program());
        assert_eq!(database.program(99).unwrap(), None);
    }

    #[test]
    fn a_program_knows_its_streams_and_the_streams_of_its_versions() {
        let database = Database::open_in_memory().unwrap();
        let stream_id = stream(&database);
        let id = database.insert_program(&program()).unwrap();
        database.link_stream_to_program(stream_id, id).unwrap();
        database
            .insert_version(&NewVersion {
                program_id: id,
                stream_id,
                period: 100.0,
                anchor_time: 0.0,
                seen_at: 0,
                on_air_from: 0.0,
            })
            .unwrap();

        assert_eq!(database.program_stream_ids(id).unwrap(), [stream_id]);
        assert_eq!(database.version_streams(id).unwrap(), [stream_id]);
        assert_eq!(database.version_streams(id + 1).unwrap(), []);
    }

    #[test]
    fn a_changed_program_keeps_its_row() {
        let database = Database::open_in_memory().unwrap();
        let id = database.insert_program(&program()).unwrap();
        let changed = NewProgram {
            artist: None,
            output_dir: "Elsewhere".to_string(),
            ..program()
        };

        database.update_program(id, &changed).unwrap();

        assert_eq!(database.program(id).unwrap().unwrap().program, changed);
    }

    #[test]
    fn programs_list_oldest_first() {
        let database = Database::open_in_memory().unwrap();
        let first = database.insert_program(&program()).unwrap();
        let second = database.insert_program(&program()).unwrap();

        let listed = database.list_programs(10, 0).unwrap();

        let ids: Vec<i64> = listed.iter().map(|row| row.id).collect();
        assert_eq!(ids, [first, second]);
    }

    #[test]
    fn no_programs_list_as_nothing() {
        let database = Database::open_in_memory().unwrap();

        assert_eq!(database.list_programs(10, 0).unwrap(), []);
    }

    #[test]
    fn a_program_lists_only_the_streams_linked_to_it_in_ascending_order() {
        let database = Database::open_in_memory().unwrap();
        let first = stream(&database);
        let unlinked = database
            .insert_stream(&NewStream {
                video_id: "BBBBBBBBBBB".to_string(),
                ..stream_values()
            })
            .unwrap();
        let third = database
            .insert_stream(&NewStream {
                video_id: "CCCCCCCCCCC".to_string(),
                ..stream_values()
            })
            .unwrap();
        let id = database.insert_program(&program()).unwrap();
        database.link_stream_to_program(third, id).unwrap();
        database.link_stream_to_program(first, id).unwrap();

        assert_eq!(database.program_stream_ids(id).unwrap(), [first, third]);
        assert_ne!(unlinked, first);
    }

    #[test]
    fn a_program_without_streams_lists_none() {
        let database = Database::open_in_memory().unwrap();
        let id = database.insert_program(&program()).unwrap();

        assert_eq!(database.program_stream_ids(id).unwrap(), []);
        assert_eq!(database.version_streams(id).unwrap(), []);
    }

    #[test]
    fn a_stream_that_observed_several_versions_is_listed_once() {
        let database = Database::open_in_memory().unwrap();
        let stream_id = stream(&database);
        let id = database.insert_program(&program()).unwrap();
        for anchor_time in [0.0, 500.0] {
            database
                .insert_version(&NewVersion {
                    program_id: id,
                    stream_id,
                    period: 100.0,
                    anchor_time,
                    seen_at: 0,
                    on_air_from: anchor_time,
                })
                .unwrap();
        }

        assert_eq!(database.version_streams(id).unwrap(), [stream_id]);
    }
}

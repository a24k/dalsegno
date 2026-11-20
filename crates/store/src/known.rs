//! Clips that recur in a program, which analysis leaves out of what it calls new (FR-K1).

use crate::database::{Database, StoreError};
use crate::features::encode;
use rusqlite::params;

/// A recurring clip, without its values.
#[derive(Debug, Clone, PartialEq)]
pub struct KnownClipRow {
    /// Row ID.
    pub id: i64,
    /// What the operator calls it.
    pub name: String,
    /// Length, in seconds.
    pub duration: f64,
    /// The feature its values belong to.
    pub feature: String,
}

/// A recurring clip to store.
#[derive(Debug, Clone, PartialEq)]
pub struct NewKnownClip<'a> {
    /// What the operator calls it.
    pub name: &'a str,
    /// Length, in seconds.
    pub duration: f64,
    /// The feature its values belong to.
    pub feature: &'a str,
    /// The values, on the feature's grid.
    pub values: &'a [f32],
}

impl Database {
    /// Returns a program's recurring clips, oldest first.
    pub fn known_clips(&self, program_id: i64) -> Result<Vec<KnownClipRow>, StoreError> {
        let mut statement = self.connection.prepare(
            "SELECT id, name, duration, feature FROM known_clips \
             WHERE program_id = ?1 ORDER BY id",
        )?;
        let rows = statement.query_map([program_id], |row| {
            Ok(KnownClipRow {
                id: row.get(0)?,
                name: row.get(1)?,
                duration: row.get(2)?,
                feature: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Stores a recurring clip for a program, and returns it.
    pub fn insert_known_clip(
        &self,
        program_id: i64,
        clip: &NewKnownClip<'_>,
    ) -> Result<KnownClipRow, StoreError> {
        self.connection.execute(
            "INSERT INTO known_clips (program_id, name, duration, feature, data) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                program_id,
                clip.name,
                clip.duration,
                clip.feature,
                encode(clip.values)
            ],
        )?;
        Ok(KnownClipRow {
            id: self.connection.last_insert_rowid(),
            name: clip.name.to_string(),
            duration: clip.duration,
            feature: clip.feature.to_string(),
        })
    }

    /// Removes a clip of a program. Returns whether the program had it.
    pub fn delete_known_clip(&self, program_id: i64, clip_id: i64) -> Result<bool, StoreError> {
        let removed = self.connection.execute(
            "DELETE FROM known_clips WHERE id = ?1 AND program_id = ?2",
            params![clip_id, program_id],
        )?;
        Ok(removed > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::programs::NewProgram;

    fn program(database: &Database) -> i64 {
        database
            .insert_program(&NewProgram {
                short_name: "Example TV".to_string(),
                channel_id: None,
                name_template: "{name}".to_string(),
                addition_suffix: "extra".to_string(),
                artist: None,
                content_date: None,
                description: None,
                output_dir: "Example TV".to_string(),
            })
            .unwrap()
    }

    fn clip(name: &str) -> NewKnownClip<'_> {
        NewKnownClip {
            name,
            duration: 2.0,
            feature: "env10",
            values: &[1.0, 2.0],
        }
    }

    #[test]
    fn a_stored_clip_is_listed_for_its_program_only() {
        let database = Database::open_in_memory().unwrap();
        let first = program(&database);
        let second = program(&database);
        let stored = database.insert_known_clip(first, &clip("Opening")).unwrap();

        assert_eq!(database.known_clips(first).unwrap(), [stored]);
        assert_eq!(database.known_clips(second).unwrap(), []);
    }

    #[test]
    fn a_clip_is_stored_as_little_endian_floats() {
        let database = Database::open_in_memory().unwrap();
        let id = program(&database);
        database.insert_known_clip(id, &clip("Opening")).unwrap();

        let data: Vec<u8> = database
            .connection()
            .query_row("SELECT data FROM known_clips", [], |row| row.get(0))
            .unwrap();

        assert_eq!(
            data,
            [1.0_f32.to_le_bytes(), 2.0_f32.to_le_bytes()].concat()
        );
    }

    #[test]
    fn a_clip_is_removed_only_from_its_own_program() {
        let database = Database::open_in_memory().unwrap();
        let first = program(&database);
        let second = program(&database);
        let stored = database.insert_known_clip(first, &clip("Opening")).unwrap();

        assert!(!database.delete_known_clip(second, stored.id).unwrap());
        assert!(database.delete_known_clip(first, stored.id).unwrap());
        assert_eq!(database.known_clips(first).unwrap(), []);
    }
}

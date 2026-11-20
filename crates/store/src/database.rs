//! The SQLite connection and its migrations.

use rusqlite::{Connection, OptionalExtension, Transaction};
use std::path::Path;
use std::time::Duration;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MIGRATIONS: [&str; 3] = [
    include_str!("../migrations/0001_initial.sql"),
    include_str!("../migrations/0002_change_tracking.sql"),
    include_str!("../migrations/0003_draft_clips.sql"),
];
const MIGRATION_COUNT: i64 = MIGRATIONS.len() as i64;

/// Why a database operation failed.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// SQLite reported an error.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The file was written by a newer version of this program.
    #[error("the database has schema version {0}, newer than this program")]
    TooNew(i64),
    /// A row holds a value this program does not recognise.
    #[error("the database holds an invalid value: {0}")]
    Invalid(&'static str),
    /// A migration left rows that point at nothing.
    #[error("a migration broke a foreign key in table {0}")]
    BrokenReference(String),
}

/// The schema version a database had when it was opened, and the one it has now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schema {
    /// The version found, which is 0 for a database that was just created.
    pub found: i64,
    /// The version after the pending migrations ran.
    pub now: i64,
}

/// One open database.
#[derive(Debug)]
pub struct Database {
    pub(crate) connection: Connection,
    schema: Schema,
}

impl Database {
    /// Opens or creates the file, then applies pending migrations.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let connection = Connection::open(path)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "NORMAL")?;
        Self::prepare(connection)
    }

    /// Opens a private database in memory, for tests.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(mut connection: Connection) -> Result<Self, StoreError> {
        // A migration may rebuild a table that others refer to, which SQLite allows only with
        // foreign keys off; the check after each one makes up for that.
        connection.pragma_update(None, "foreign_keys", "OFF")?;
        let schema = migrate(&mut connection)?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        Ok(Self { connection, schema })
    }

    /// Returns the schema version found at open and the one left.
    pub fn schema(&self) -> Schema {
        self.schema
    }

    /// Returns the connection, for statements no repository covers yet.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Runs `work` in one transaction, which commits only if it returns `Ok`.
    pub fn transaction<T>(
        &mut self,
        work: impl FnOnce(&Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let transaction = self.connection.transaction()?;
        let result = work(&transaction)?;
        transaction.commit()?;
        Ok(result)
    }
}

fn migrate(connection: &mut Connection) -> Result<Schema, StoreError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > MIGRATION_COUNT {
        return Err(StoreError::TooNew(version));
    }
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let transaction = connection.transaction()?;
        transaction.execute_batch(migration)?;
        transaction.pragma_update(None, "user_version", index as i64 + 1)?;
        check_foreign_keys(&transaction)?;
        transaction.commit()?;
    }
    Ok(Schema {
        found: version,
        now: MIGRATION_COUNT,
    })
}

fn check_foreign_keys(connection: &Connection) -> Result<(), StoreError> {
    let broken: Option<String> = connection
        .query_row("PRAGMA foreign_key_check", [], |row| row.get(0))
        .optional()?;
    match broken {
        Some(table) => Err(StoreError::BrokenReference(table)),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(database: &Database) -> i64 {
        database
            .connection()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn migrations_set_the_schema_version() {
        let database = Database::open_in_memory().unwrap();

        assert_eq!(version(&database), MIGRATION_COUNT);
    }

    #[test]
    fn a_new_file_reports_the_versions_it_migrated_between() {
        let dir = tempfile::tempdir().unwrap();

        let database = Database::open(&dir.path().join("test.sqlite3")).unwrap();

        assert_eq!(
            database.schema(),
            Schema {
                found: 0,
                now: MIGRATION_COUNT
            }
        );
    }

    #[test]
    fn a_reopened_file_reports_the_same_version_twice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite3");
        Database::open(&path).unwrap();

        let database = Database::open(&path).unwrap();

        assert_eq!(
            database.schema(),
            Schema {
                found: MIGRATION_COUNT,
                now: MIGRATION_COUNT
            }
        );
    }

    #[test]
    fn the_design_tables_exist() {
        let database = Database::open_in_memory().unwrap();

        let tables: i64 = database
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(tables, 19);
    }

    #[test]
    fn reopening_keeps_data_and_uses_wal() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite3");
        Database::open(&path)
            .unwrap()
            .connection()
            .execute("INSERT INTO settings VALUES ('k', '1')", [])
            .unwrap();

        let database = Database::open(&path).unwrap();

        let value: String = database
            .connection()
            .query_row("SELECT value FROM settings", [], |row| row.get(0))
            .unwrap();
        let mode: String = database
            .connection()
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!((value.as_str(), mode.as_str()), ("1", "wal"));
    }

    #[test]
    fn foreign_keys_are_enforced() {
        let database = Database::open_in_memory().unwrap();

        let result = database.connection().execute(
            "INSERT INTO fragments VALUES (99, 140, 1, 0.0, 5.0, 10, 0)",
            [],
        );

        assert!(result.is_err());
    }

    #[test]
    fn a_newer_database_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite3");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", MIGRATION_COUNT + 1)
            .unwrap();

        assert!(matches!(Database::open(&path), Err(StoreError::TooNew(_))));
    }

    #[test]
    fn a_failed_transaction_leaves_nothing() {
        let mut database = Database::open_in_memory().unwrap();

        let result: Result<(), StoreError> = database.transaction(|transaction| {
            transaction.execute("INSERT INTO settings VALUES ('k', '1')", [])?;
            Err(StoreError::Invalid("test"))
        });

        assert!(result.is_err());
        let rows: i64 = database
            .connection()
            .query_row("SELECT COUNT(*) FROM settings", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn a_clip_and_its_hold_survive_the_draft_migration() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite3");
        let old = Connection::open(&path).unwrap();
        for migration in &MIGRATIONS[..2] {
            old.execute_batch(migration).unwrap();
        }
        old.pragma_update(None, "user_version", 2).unwrap();
        old.execute_batch(
            "INSERT INTO streams (id, video_id, title, state, video_itag, audio_itag,
                                  fragment_len, registered_at)
               VALUES (1, 'AAAAAAAAAAA', 'Example', 'registered', 248, 140, 5.0, 0);
             INSERT INTO clips (id, name, kind, stream_id, video_itag, audio_itag, sq_first,
                                sq_last, duration, metadata)
               VALUES (7, 'Example', 'addition', 1, 248, 140, 10, 20, 50.0, '{}');
             INSERT INTO retention_holds (stream_id, itag, sq_first, sq_last, clip_id)
               VALUES (1, 248, 10, 20, 7);",
        )
        .unwrap();
        drop(old);

        let database = Database::open(&path).unwrap();

        let (state, holds): (String, i64) = database
            .connection()
            .query_row(
                "SELECT (SELECT state FROM clips WHERE id = 7),
                        (SELECT COUNT(*) FROM retention_holds WHERE clip_id = 7)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (version(&database), state.as_str(), holds),
            (3, "exported", 1)
        );
    }
}

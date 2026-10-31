//! The SQLite connection and its migrations.

use rusqlite::{Connection, Transaction};
use std::path::Path;
use std::time::Duration;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const MIGRATIONS: [&str; 1] = [include_str!("../migrations/0001_initial.sql")];
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
}

/// One open database.
#[derive(Debug)]
pub struct Database {
    pub(crate) connection: Connection,
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
        connection.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&mut connection)?;
        Ok(Self { connection })
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

fn migrate(connection: &mut Connection) -> Result<(), StoreError> {
    let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if version > MIGRATION_COUNT {
        return Err(StoreError::TooNew(version));
    }
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let transaction = connection.transaction()?;
        transaction.execute_batch(migration)?;
        transaction.pragma_update(None, "user_version", index as i64 + 1)?;
        transaction.commit()?;
    }
    Ok(())
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
}

//! Operational settings that override the file and environment (D-24). Values are JSON.

use crate::database::{Database, StoreError};
use rusqlite::params;

impl Database {
    /// Returns every override, by key.
    pub fn settings(&self) -> Result<Vec<(String, String)>, StoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT key, value FROM settings ORDER BY key")?;
        let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Sets an override.
    pub fn put_setting(&self, key: &str, value: &str) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) \
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Removes an override, so the value from the file or environment applies again.
    pub fn delete_setting(&self, key: &str) -> Result<(), StoreError> {
        self.connection
            .execute("DELETE FROM settings WHERE key = ?1", [key])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_override_is_set_replaced_and_removed() {
        let database = Database::open_in_memory().unwrap();

        database.put_setting("concurrency", "2").unwrap();
        database.put_setting("daily_summary", "\"09:00\"").unwrap();
        database.put_setting("concurrency", "3").unwrap();
        let set = database.settings().unwrap();
        database.delete_setting("concurrency").unwrap();
        let left = database.settings().unwrap();

        assert_eq!(
            set,
            [
                ("concurrency".to_string(), "3".to_string()),
                ("daily_summary".to_string(), "\"09:00\"".to_string()),
            ]
        );
        assert_eq!(
            left,
            [("daily_summary".to_string(), "\"09:00\"".to_string())]
        );
    }
}

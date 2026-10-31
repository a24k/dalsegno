//! Explicit holes in the recording (FR-A11).

use crate::database::{Database, StoreError};
use rusqlite::params;

/// What became of a gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapStatus {
    /// Still to be filled.
    Open,
    /// Fetched.
    Filled,
    /// Left the DVR before it could be fetched.
    Lost,
}

impl GapStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Filled => "filled",
            Self::Lost => "lost",
        }
    }
}

/// One open gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    /// Row ID.
    pub id: i64,
    /// Stream.
    pub stream_id: i64,
    /// Format.
    pub itag: u32,
    /// First missing `sq`.
    pub sq_first: u64,
    /// Last missing `sq`.
    pub sq_last: u64,
}

impl Database {
    /// Records an open gap, and returns its row ID.
    pub fn add_gap(
        &self,
        stream_id: i64,
        itag: u32,
        sq_first: u64,
        sq_last: u64,
    ) -> Result<i64, StoreError> {
        self.connection.execute(
            "INSERT INTO gaps (stream_id, itag, sq_first, sq_last, status) \
             VALUES (?1, ?2, ?3, ?4, 'open')",
            params![stream_id, itag, sq_first as i64, sq_last as i64],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Returns open gaps of a track, oldest first (FR-A6).
    pub fn open_gaps(&self, stream_id: i64, itag: u32) -> Result<Vec<Gap>, StoreError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT id, sq_first, sq_last FROM gaps \
             WHERE stream_id = ?1 AND itag = ?2 AND status = 'open' ORDER BY sq_first",
        )?;
        let gaps = statement
            .query_map(params![stream_id, itag], |row| {
                Ok(Gap {
                    id: row.get(0)?,
                    stream_id,
                    itag,
                    sq_first: row.get::<_, i64>(1)? as u64,
                    sq_last: row.get::<_, i64>(2)? as u64,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(gaps)
    }

    /// Changes a gap's status.
    pub fn set_gap_status(&self, id: i64, status: GapStatus) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE gaps SET status = ?2 WHERE id = ?1",
            params![id, status.as_str()],
        )?;
        Ok(())
    }

    /// Replaces a gap with what remains of it, which keeps the rest open.
    pub fn shrink_gap(&self, id: i64, sq_first: u64, sq_last: u64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE gaps SET sq_first = ?2, sq_last = ?3 WHERE id = ?1",
            params![id, sq_first as i64, sq_last as i64],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fragments::tests::stream;

    #[test]
    fn open_gaps_come_oldest_first() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        database.add_gap(id, 140, 50, 60).unwrap();
        database.add_gap(id, 140, 10, 20).unwrap();
        database.add_gap(id, 248, 1, 2).unwrap();

        let gaps = database.open_gaps(id, 140).unwrap();

        let ranges: Vec<_> = gaps.iter().map(|g| (g.sq_first, g.sq_last)).collect();
        assert_eq!(ranges, [(10, 20), (50, 60)]);
    }

    #[test]
    fn closed_gaps_are_not_open() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        let filled = database.add_gap(id, 140, 1, 2).unwrap();
        let lost = database.add_gap(id, 140, 3, 4).unwrap();

        database.set_gap_status(filled, GapStatus::Filled).unwrap();
        database.set_gap_status(lost, GapStatus::Lost).unwrap();

        assert_eq!(database.open_gaps(id, 140).unwrap(), []);
    }

    #[test]
    fn shrinking_keeps_the_rest_open() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        let gap = database.add_gap(id, 140, 1, 9).unwrap();

        database.shrink_gap(gap, 5, 9).unwrap();

        assert_eq!(database.open_gaps(id, 140).unwrap()[0].sq_first, 5);
    }
}

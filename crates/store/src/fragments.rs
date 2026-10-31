//! Fragment rows and head samples: the measurements that map `sq` to time.

use crate::database::{Database, StoreError};
use rusqlite::{OptionalExtension, Row, params};

/// One stored fragment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FragmentRow {
    /// Stream.
    pub stream_id: i64,
    /// Format.
    pub itag: u32,
    /// Sequence number.
    pub sq: u64,
    /// Stream time of the first sample, read from the fragment bytes.
    pub stream_time: f64,
    /// Duration in seconds.
    pub duration: f64,
    /// File size in bytes.
    pub size: u64,
    /// Fetch time, in epoch milliseconds.
    pub fetched_at: i64,
}

/// One observation of the head.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadSample {
    /// Stream.
    pub stream_id: i64,
    /// Server wall clock, in epoch milliseconds.
    pub observed_at: i64,
    /// Latest `sq`.
    pub head_sq: u64,
    /// Stream time of the head.
    pub head_time: f64,
}

fn fragment_from_row(row: &Row<'_>) -> rusqlite::Result<FragmentRow> {
    Ok(FragmentRow {
        stream_id: row.get(0)?,
        itag: row.get(1)?,
        sq: row.get::<_, i64>(2)? as u64,
        stream_time: row.get(3)?,
        duration: row.get(4)?,
        size: row.get::<_, i64>(5)? as u64,
        fetched_at: row.get(6)?,
    })
}

impl Database {
    /// Adds a fragment, or replaces the row of a refetched one.
    pub fn put_fragment(&self, fragment: &FragmentRow) -> Result<(), StoreError> {
        self.connection
            .prepare_cached("INSERT OR REPLACE INTO fragments VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?
            .execute(params![
                fragment.stream_id,
                fragment.itag,
                fragment.sq as i64,
                fragment.stream_time,
                fragment.duration,
                fragment.size as i64,
                fragment.fetched_at,
            ])?;
        Ok(())
    }

    /// Returns the rows of `first..=last` in `sq` order.
    pub fn fragments_in_range(
        &self,
        stream_id: i64,
        itag: u32,
        first: u64,
        last: u64,
    ) -> Result<Vec<FragmentRow>, StoreError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT stream_id, itag, sq, stream_time, duration, size, fetched_at FROM fragments \
             WHERE stream_id = ?1 AND itag = ?2 AND sq BETWEEN ?3 AND ?4 ORDER BY sq",
        )?;
        let rows = statement
            .query_map(
                params![stream_id, itag, first as i64, last as i64],
                fragment_from_row,
            )?
            .collect::<Result<_, _>>()?;
        Ok(rows)
    }

    /// Returns the lowest and highest stored `sq`.
    pub fn fragment_bounds(
        &self,
        stream_id: i64,
        itag: u32,
    ) -> Result<Option<(u64, u64)>, StoreError> {
        let bounds: (Option<i64>, Option<i64>) = self.connection.query_row(
            "SELECT MIN(sq), MAX(sq) FROM fragments WHERE stream_id = ?1 AND itag = ?2",
            params![stream_id, itag],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok(bounds.0.zip(bounds.1).map(|(a, b)| (a as u64, b as u64)))
    }

    /// Removes a row whose file is gone. Features stay, as they do not need the file.
    pub fn delete_fragment(&self, stream_id: i64, itag: u32, sq: u64) -> Result<(), StoreError> {
        self.connection.execute(
            "DELETE FROM fragments WHERE stream_id = ?1 AND itag = ?2 AND sq = ?3",
            params![stream_id, itag, sq as i64],
        )?;
        Ok(())
    }

    /// Returns each stream and format that has fragments.
    pub fn fragment_tracks(&self) -> Result<Vec<(i64, u32)>, StoreError> {
        let mut statement = self
            .connection
            .prepare_cached("SELECT DISTINCT stream_id, itag FROM fragments")?;
        let tracks = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        Ok(tracks)
    }

    /// Stores a head sample unless one for the same instant exists.
    pub fn put_head_sample(&self, sample: &HeadSample) -> Result<(), StoreError> {
        self.connection.execute(
            "INSERT OR IGNORE INTO head_samples VALUES (?1, ?2, ?3, ?4)",
            params![
                sample.stream_id,
                sample.observed_at,
                sample.head_sq as i64,
                sample.head_time,
            ],
        )?;
        Ok(())
    }

    /// Returns the most recent head sample.
    pub fn latest_head_sample(&self, stream_id: i64) -> Result<Option<HeadSample>, StoreError> {
        Ok(self
            .connection
            .query_row(
                "SELECT observed_at, head_sq, head_time FROM head_samples \
                 WHERE stream_id = ?1 ORDER BY observed_at DESC LIMIT 1",
                [stream_id],
                |row| {
                    Ok(HeadSample {
                        stream_id,
                        observed_at: row.get(0)?,
                        head_sq: row.get::<_, i64>(1)? as u64,
                        head_time: row.get(2)?,
                    })
                },
            )
            .optional()?)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::streams::tests::new_stream;

    pub(crate) fn stream(database: &Database) -> i64 {
        database.insert_stream(&new_stream("abcDEF123_-")).unwrap()
    }

    pub(crate) fn fragment(stream_id: i64, itag: u32, sq: u64) -> FragmentRow {
        FragmentRow {
            stream_id,
            itag,
            sq,
            stream_time: sq as f64 * 5.0,
            duration: 5.0,
            size: 100,
            fetched_at: 1_000,
        }
    }

    #[test]
    fn range_is_ordered_and_bounded() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        for sq in [7, 5, 6, 9] {
            database.put_fragment(&fragment(id, 140, sq)).unwrap();
        }
        database.put_fragment(&fragment(id, 248, 6)).unwrap();

        let rows = database.fragments_in_range(id, 140, 6, 8).unwrap();

        assert_eq!(rows, [fragment(id, 140, 6), fragment(id, 140, 7)]);
    }

    #[test]
    fn refetching_replaces_the_row() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        database.put_fragment(&fragment(id, 140, 5)).unwrap();
        let newer = FragmentRow {
            fetched_at: 2_000,
            ..fragment(id, 140, 5)
        };

        database.put_fragment(&newer).unwrap();

        assert_eq!(database.fragments_in_range(id, 140, 5, 5).unwrap(), [newer]);
    }

    #[test]
    fn bounds_cover_stored_fragments() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);

        assert_eq!(database.fragment_bounds(id, 140).unwrap(), None);

        database.put_fragment(&fragment(id, 140, 9)).unwrap();
        database.put_fragment(&fragment(id, 140, 5)).unwrap();

        assert_eq!(database.fragment_bounds(id, 140).unwrap(), Some((5, 9)));
    }

    #[test]
    fn deleting_a_row_leaves_the_track_when_others_remain() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        database.put_fragment(&fragment(id, 140, 5)).unwrap();
        database.put_fragment(&fragment(id, 140, 6)).unwrap();
        database.put_fragment(&fragment(id, 248, 6)).unwrap();

        database.delete_fragment(id, 248, 6).unwrap();
        database.delete_fragment(id, 140, 5).unwrap();

        assert_eq!(database.fragment_tracks().unwrap(), [(id, 140)]);
    }

    #[test]
    fn latest_head_sample_is_the_newest() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        let sample = |observed_at, head_sq| HeadSample {
            stream_id: id,
            observed_at,
            head_sq,
            head_time: 1.5,
        };

        assert_eq!(database.latest_head_sample(id).unwrap(), None);

        database.put_head_sample(&sample(2_000, 20)).unwrap();
        database.put_head_sample(&sample(1_000, 10)).unwrap();
        database.put_head_sample(&sample(2_000, 99)).unwrap();

        assert_eq!(
            database.latest_head_sample(id).unwrap(),
            Some(sample(2_000, 20))
        );
    }
}

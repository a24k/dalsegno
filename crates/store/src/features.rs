//! Envelope rows and the runs that produce them.

use crate::database::{Database, StoreError};
use rusqlite::{OptionalExtension, params};

const F32_BYTES: usize = size_of::<f32>();

/// The feature name of the 10 Hz envelope.
pub const ENVELOPE_FEATURE: &str = "env10";

/// One contiguous stretch of processed audio.
#[derive(Debug, Clone, PartialEq)]
pub struct FeatureRun {
    /// Row ID.
    pub id: i64,
    /// Stream.
    pub stream_id: i64,
    /// First `sq` of the run.
    pub sq_first: u64,
    /// The `sq` to process next.
    pub next_sq: u64,
    /// Stream time of the run's first sample.
    pub anchor_time: f64,
}

/// The values produced while processing one fragment.
#[derive(Debug, Clone, PartialEq)]
pub struct EnvelopeRow {
    /// The fragment.
    pub sq: u64,
    /// Grid index of the first value.
    pub first_bin: u64,
    /// The values, which are contiguous on the grid.
    pub values: Vec<f32>,
}

fn encode(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn decode(bytes: &[u8]) -> Result<Vec<f32>, StoreError> {
    if !bytes.len().is_multiple_of(F32_BYTES) {
        return Err(StoreError::Invalid("envelope blob length"));
    }
    Ok(bytes
        .as_chunks::<F32_BYTES>()
        .0
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect())
}

impl Database {
    /// Creates a run, and returns it.
    pub fn insert_feature_run(
        &self,
        stream_id: i64,
        sq_first: u64,
        anchor_time: f64,
    ) -> Result<FeatureRun, StoreError> {
        self.connection.execute(
            "INSERT INTO feature_runs (stream_id, feature, sq_first, next_sq, anchor_time) \
             VALUES (?1, ?2, ?3, ?3, ?4)",
            params![stream_id, ENVELOPE_FEATURE, sq_first as i64, anchor_time],
        )?;
        Ok(FeatureRun {
            id: self.connection.last_insert_rowid(),
            stream_id,
            sq_first,
            next_sq: sq_first,
            anchor_time,
        })
    }

    /// Returns a stream's runs in `sq` order.
    pub fn feature_runs(&self, stream_id: i64) -> Result<Vec<FeatureRun>, StoreError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT id, sq_first, next_sq, anchor_time FROM feature_runs \
             WHERE stream_id = ?1 AND feature = ?2 ORDER BY sq_first",
        )?;
        let runs = statement
            .query_map(params![stream_id, ENVELOPE_FEATURE], |row| {
                Ok(FeatureRun {
                    id: row.get(0)?,
                    stream_id,
                    sq_first: row.get::<_, i64>(1)? as u64,
                    next_sq: row.get::<_, i64>(2)? as u64,
                    anchor_time: row.get(3)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(runs)
    }

    /// Stores a fragment's values and moves the run past it, in one transaction.
    pub fn advance_feature_run(
        &mut self,
        run: &FeatureRun,
        row: &EnvelopeRow,
    ) -> Result<(), StoreError> {
        self.transaction(|transaction| {
            transaction.execute(
                "INSERT OR REPLACE INTO envelope (stream_id, sq, first_bin, feature, data) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    run.stream_id,
                    row.sq as i64,
                    row.first_bin as i64,
                    ENVELOPE_FEATURE,
                    encode(&row.values),
                ],
            )?;
            transaction.execute(
                "UPDATE feature_runs SET next_sq = ?2 WHERE id = ?1",
                params![run.id, row.sq as i64 + 1],
            )?;
            Ok(())
        })
    }

    /// Joins `following` onto `run`, which has processed up to it.
    pub fn merge_feature_runs(
        &mut self,
        run: &FeatureRun,
        following: &FeatureRun,
    ) -> Result<(), StoreError> {
        self.transaction(|transaction| {
            transaction.execute(
                "UPDATE feature_runs SET next_sq = MAX(next_sq, ?2) WHERE id = ?1",
                params![run.id, following.next_sq as i64],
            )?;
            transaction.execute("DELETE FROM feature_runs WHERE id = ?1", [following.id])?;
            Ok(())
        })
    }

    /// Returns the rows of `first..=last` in `sq` order.
    pub fn envelope_rows(
        &self,
        stream_id: i64,
        first: u64,
        last: u64,
    ) -> Result<Vec<EnvelopeRow>, StoreError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT sq, first_bin, data FROM envelope \
             WHERE stream_id = ?1 AND feature = ?2 AND sq BETWEEN ?3 AND ?4 ORDER BY sq",
        )?;
        let raw: Vec<(i64, i64, Vec<u8>)> = statement
            .query_map(
                params![stream_id, ENVELOPE_FEATURE, first as i64, last as i64],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?
            .collect::<Result<_, _>>()?;
        raw.into_iter()
            .map(|(sq, first_bin, data)| {
                Ok(EnvelopeRow {
                    sq: sq as u64,
                    first_bin: first_bin as u64,
                    values: decode(&data)?,
                })
            })
            .collect()
    }

    /// Returns the grid index just past the row of `sq`.
    pub fn envelope_end_bin(&self, stream_id: i64, sq: u64) -> Result<Option<u64>, StoreError> {
        let row: Option<(i64, i64)> = self
            .connection
            .query_row(
                "SELECT first_bin, LENGTH(data) FROM envelope \
                 WHERE stream_id = ?1 AND feature = ?2 AND sq = ?3",
                params![stream_id, ENVELOPE_FEATURE, sq as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(row.map(|(first_bin, bytes)| first_bin as u64 + bytes as u64 / F32_BYTES as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fragments::tests::stream;

    fn row(sq: u64, first_bin: u64, values: &[f32]) -> EnvelopeRow {
        EnvelopeRow {
            sq,
            first_bin,
            values: values.to_vec(),
        }
    }

    #[test]
    fn advancing_stores_the_row_and_moves_the_run() {
        let mut database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        let run = database.insert_feature_run(id, 10, 50.0).unwrap();
        let stored = row(10, 500, &[1.5, -2.5, 0.0]);

        database.advance_feature_run(&run, &stored).unwrap();

        assert_eq!(database.envelope_rows(id, 0, 99).unwrap(), [stored]);
        assert_eq!(database.feature_runs(id).unwrap()[0].next_sq, 11);
    }

    #[test]
    fn recomputing_a_fragment_replaces_its_row() {
        let mut database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        let run = database.insert_feature_run(id, 10, 50.0).unwrap();
        database
            .advance_feature_run(&run, &row(10, 500, &[1.0]))
            .unwrap();
        let replacement = row(10, 499, &[2.0, 3.0]);

        database.advance_feature_run(&run, &replacement).unwrap();

        assert_eq!(database.envelope_rows(id, 10, 10).unwrap(), [replacement]);
    }

    #[test]
    fn end_bin_follows_the_last_value() {
        let mut database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        let run = database.insert_feature_run(id, 10, 50.0).unwrap();
        database
            .advance_feature_run(&run, &row(10, 500, &[1.0, 2.0]))
            .unwrap();

        assert_eq!(database.envelope_end_bin(id, 10).unwrap(), Some(502));
        assert_eq!(database.envelope_end_bin(id, 11).unwrap(), None);
    }

    #[test]
    fn merging_extends_the_first_run_and_removes_the_second() {
        let mut database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        let first = database.insert_feature_run(id, 10, 50.0).unwrap();
        let second = database.insert_feature_run(id, 20, 100.0).unwrap();
        database
            .advance_feature_run(&second, &row(20, 1000, &[1.0]))
            .unwrap();
        let second = database.feature_runs(id).unwrap()[1].clone();

        database.merge_feature_runs(&first, &second).unwrap();

        let runs = database.feature_runs(id).unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!((runs[0].sq_first, runs[0].next_sq), (10, 21));
    }

    #[test]
    fn a_damaged_blob_is_an_error() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        database
            .connection()
            .execute(
                "INSERT INTO envelope (stream_id, sq, first_bin, data) VALUES (?1, 1, 1, x'0011')",
                [id],
            )
            .unwrap();

        assert!(matches!(
            database.envelope_rows(id, 1, 1),
            Err(StoreError::Invalid(_))
        ));
    }
}

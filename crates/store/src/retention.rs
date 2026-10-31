//! Retention holds and what the sweep may remove (D-11, D-12).

use crate::database::{Database, StoreError};
use crate::features::ENVELOPE_FEATURE;
use rusqlite::params;

const DAY_MS: i64 = 24 * 3600 * 1000;
const HOUR_MS: i64 = 3600 * 1000;
const BOOKKEEPING_DAYS: i64 = 30;

/// What keeps a hold alive. Exactly one owner, as the schema requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldOwner {
    /// A version, whose hold never expires.
    Version(i64),
    /// A change.
    Change(i64),
    /// A clip.
    Clip(i64),
}

/// A range to keep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewHold {
    /// Stream.
    pub stream_id: i64,
    /// Format.
    pub itag: u32,
    /// First `sq`.
    pub sq_first: u64,
    /// Last `sq`.
    pub sq_last: u64,
    /// What the hold belongs to.
    pub owner: HoldOwner,
}

/// A fragment the sweep may remove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SweepItem {
    /// Stream.
    pub stream_id: i64,
    /// Format.
    pub itag: u32,
    /// Sequence number.
    pub sq: u64,
}

impl Database {
    /// Creates a hold, which lasts while its owner is open.
    pub fn insert_hold(&self, hold: &NewHold) -> Result<i64, StoreError> {
        let (version, change, clip) = match hold.owner {
            HoldOwner::Version(id) => (Some(id), None, None),
            HoldOwner::Change(id) => (None, Some(id), None),
            HoldOwner::Clip(id) => (None, None, Some(id)),
        };
        self.connection.execute(
            "INSERT INTO retention_holds \
             (stream_id, itag, sq_first, sq_last, version_id, change_id, clip_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                hold.stream_id,
                hold.itag,
                hold.sq_first as i64,
                hold.sq_last as i64,
                version,
                change,
                clip,
            ],
        )?;
        Ok(self.connection.last_insert_rowid())
    }

    /// Sets when a closed owner's hold ends.
    pub fn expire_hold(&self, id: i64, expires_at: i64) -> Result<(), StoreError> {
        self.connection.execute(
            "UPDATE retention_holds SET expires_at = ?2 WHERE id = ?1",
            params![id, expires_at],
        )?;
        Ok(())
    }

    /// Returns fragments past retention and under no live hold, in key order after `after`.
    ///
    /// Audio has base retention until `audio_cutoff`. Other formats have none.
    pub fn sweep_candidates(
        &self,
        audio_cutoff: i64,
        now: i64,
        after: Option<SweepItem>,
        limit: u32,
    ) -> Result<Vec<SweepItem>, StoreError> {
        let (after_stream, after_itag, after_sq) = after.map_or((i64::MIN, 0, -1), |item| {
            (item.stream_id, item.itag, item.sq as i64)
        });
        let mut statement = self.connection.prepare_cached(
            "SELECT f.stream_id, f.itag, f.sq FROM fragments f \
             JOIN streams s ON s.id = f.stream_id \
             WHERE (f.stream_id, f.itag, f.sq) > (?3, ?4, ?5) \
             AND (f.itag != s.audio_itag OR f.fetched_at < ?1) \
             AND NOT EXISTS (SELECT 1 FROM retention_holds h \
               WHERE h.stream_id = f.stream_id AND h.itag = f.itag \
               AND f.sq BETWEEN h.sq_first AND h.sq_last \
               AND (h.expires_at IS NULL OR h.expires_at > ?2)) \
             ORDER BY f.stream_id, f.itag, f.sq LIMIT ?6",
        )?;
        let items = statement
            .query_map(
                params![audio_cutoff, now, after_stream, after_itag, after_sq, limit],
                |row| {
                    Ok(SweepItem {
                        stream_id: row.get(0)?,
                        itag: row.get(1)?,
                        sq: row.get::<_, i64>(2)? as u64,
                    })
                },
            )?
            .collect::<Result<_, _>>()?;
        Ok(items)
    }

    /// Removes a swept fragment's row, and its envelope when it is the stream's audio.
    pub fn delete_swept(&mut self, item: &SweepItem) -> Result<(), StoreError> {
        self.transaction(|transaction| {
            transaction.execute(
                "DELETE FROM fragments WHERE stream_id = ?1 AND itag = ?2 AND sq = ?3",
                params![item.stream_id, item.itag, item.sq as i64],
            )?;
            transaction.execute(
                "DELETE FROM envelope WHERE stream_id = ?1 AND feature = ?4 AND sq = ?3 \
                 AND ?2 = (SELECT audio_itag FROM streams WHERE id = ?1)",
                params![item.stream_id, item.itag, item.sq as i64, ENVELOPE_FEATURE],
            )?;
            Ok(())
        })
    }

    /// Prunes notifications, jobs and head samples by their rules in the retention table.
    pub fn prune_bookkeeping(&mut self, now: i64) -> Result<(), StoreError> {
        let cutoff = now - BOOKKEEPING_DAYS * DAY_MS;
        self.transaction(|transaction| {
            transaction.execute(
                "DELETE FROM notifications WHERE sent_at IS NOT NULL AND sent_at < ?1",
                [cutoff],
            )?;
            transaction.execute(
                "DELETE FROM jobs WHERE state = 'succeeded' AND finished_at < ?1",
                [cutoff],
            )?;
            transaction.execute(
                "DELETE FROM head_samples WHERE observed_at < ?1 AND observed_at NOT IN \
                 (SELECT MIN(observed_at) FROM head_samples \
                  GROUP BY stream_id, observed_at / ?2)",
                params![cutoff, HOUR_MS],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::EnvelopeRow;
    use crate::fragments::tests::{fragment, stream};
    use crate::fragments::{FragmentRow, HeadSample};

    const AUDIO: u32 = 140;
    const VIDEO: u32 = 248;
    const NOW: i64 = 100 * DAY_MS;
    const CUTOFF: i64 = 86 * DAY_MS;

    fn change(database: &Database, stream_id: i64) -> i64 {
        database
            .connection()
            .execute_batch(
                "INSERT INTO programs (id, short_name, name_template, addition_suffix, output_dir) \
                 VALUES (1, 'Example TV', 't', 's', '/o');",
            )
            .unwrap();
        database
            .connection()
            .execute(
                "INSERT INTO changes (program_id, stream_id, detected_at, state) \
                 VALUES (1, ?1, 0, 'dismissed')",
                [stream_id],
            )
            .unwrap();
        database.connection().last_insert_rowid()
    }

    fn hold(stream_id: i64, itag: u32, owner: HoldOwner) -> NewHold {
        NewHold {
            stream_id,
            itag,
            sq_first: 5,
            sq_last: 6,
            owner,
        }
    }

    fn old(stream_id: i64, itag: u32, sq: u64) -> FragmentRow {
        FragmentRow {
            fetched_at: CUTOFF - 1,
            ..fragment(stream_id, itag, sq)
        }
    }

    fn fresh(stream_id: i64, itag: u32, sq: u64) -> FragmentRow {
        FragmentRow {
            fetched_at: CUTOFF,
            ..fragment(stream_id, itag, sq)
        }
    }

    fn sqs(database: &Database) -> Vec<u64> {
        let mut found: Vec<u64> = database
            .sweep_candidates(CUTOFF, NOW, None, 100)
            .unwrap()
            .iter()
            .map(|item| item.sq)
            .collect();
        found.sort_unstable();
        found
    }

    #[test]
    fn candidates_page_in_key_order() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        for sq in [3, 1, 2] {
            database.put_fragment(&old(id, AUDIO, sq)).unwrap();
        }

        let first = database.sweep_candidates(CUTOFF, NOW, None, 2).unwrap();
        let rest = database
            .sweep_candidates(CUTOFF, NOW, first.last().copied(), 2)
            .unwrap();

        let page = |items: &[SweepItem]| items.iter().map(|item| item.sq).collect::<Vec<_>>();
        assert_eq!((page(&first), page(&rest)), (vec![1, 2], vec![3]));
    }

    #[test]
    fn audio_is_kept_until_the_cutoff() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        database.put_fragment(&old(id, AUDIO, 1)).unwrap();
        database.put_fragment(&fresh(id, AUDIO, 2)).unwrap();

        assert_eq!(sqs(&database), [1]);
    }

    #[test]
    fn video_has_no_base_retention() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        database.put_fragment(&fresh(id, VIDEO, 1)).unwrap();

        assert_eq!(sqs(&database), [1]);
    }

    #[test]
    fn a_hold_protects_only_its_range_and_track() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        let owner = HoldOwner::Change(change(&database, id));
        database.insert_hold(&hold(id, VIDEO, owner)).unwrap();
        for sq in 4..=7 {
            database.put_fragment(&old(id, VIDEO, sq)).unwrap();
        }
        database.put_fragment(&old(id, AUDIO, 5)).unwrap();

        let items = database.sweep_candidates(CUTOFF, NOW, None, 100).unwrap();

        assert_eq!(items.len(), 3);
        assert!(
            !items
                .iter()
                .any(|item| item.itag == VIDEO && (5..=6).contains(&item.sq))
        );
    }

    #[test]
    fn a_hold_ends_when_it_expires() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        let owner = HoldOwner::Change(change(&database, id));
        let held = database.insert_hold(&hold(id, VIDEO, owner)).unwrap();
        database.put_fragment(&old(id, VIDEO, 5)).unwrap();

        database.expire_hold(held, NOW + 1).unwrap();
        assert_eq!(sqs(&database), []);

        database.expire_hold(held, NOW).unwrap();
        assert_eq!(sqs(&database), [5]);
    }

    #[test]
    fn a_fragment_with_several_owners_needs_all_of_them_gone() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        let owner = HoldOwner::Change(change(&database, id));
        let first = database.insert_hold(&hold(id, VIDEO, owner)).unwrap();
        let second = database.insert_hold(&hold(id, VIDEO, owner)).unwrap();
        database.put_fragment(&old(id, VIDEO, 5)).unwrap();

        database.expire_hold(first, NOW).unwrap();
        assert_eq!(sqs(&database), []);

        database.expire_hold(second, NOW).unwrap();
        assert_eq!(sqs(&database), [5]);
    }

    #[test]
    fn a_hold_needs_a_real_owner() {
        let database = Database::open_in_memory().unwrap();
        let id = stream(&database);

        assert!(
            database
                .insert_hold(&hold(id, VIDEO, HoldOwner::Version(99)))
                .is_err()
        );
        assert!(
            database
                .insert_hold(&hold(id, VIDEO, HoldOwner::Clip(99)))
                .is_err()
        );
    }

    #[test]
    fn deleting_audio_takes_its_envelope_but_video_does_not() {
        let mut database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        let run = database.insert_feature_run(id, 1, 0.0).unwrap();
        let row = EnvelopeRow {
            sq: 1,
            first_bin: 0,
            values: vec![1.0],
        };
        database.advance_feature_run(&run, &row).unwrap();
        database.put_fragment(&old(id, VIDEO, 1)).unwrap();
        database.put_fragment(&old(id, AUDIO, 1)).unwrap();
        let swept = |itag| SweepItem {
            stream_id: id,
            itag,
            sq: 1,
        };

        database.delete_swept(&swept(VIDEO)).unwrap();
        assert_eq!(database.envelope_rows(id, 1, 1).unwrap().len(), 1);

        database.delete_swept(&swept(AUDIO)).unwrap();
        assert_eq!(database.envelope_rows(id, 1, 1).unwrap(), []);
        assert_eq!(database.fragment_bounds(id, AUDIO).unwrap(), None);
        assert_eq!(ENVELOPE_FEATURE, "env10");
    }

    #[test]
    fn bookkeeping_follows_the_retention_table() {
        let mut database = Database::open_in_memory().unwrap();
        let id = stream(&database);
        let old_time = NOW - 31 * DAY_MS;
        let sample = |observed_at| HeadSample {
            stream_id: id,
            observed_at,
            head_sq: 1,
            head_time: 1.0,
        };
        for offset in [0, 60_000, HOUR_MS] {
            database
                .put_head_sample(&sample(old_time + offset))
                .unwrap();
        }
        database.put_head_sample(&sample(NOW)).unwrap();
        let connection = database.connection();
        connection
            .execute(
                "INSERT INTO notifications (kind, subject, body, created_at, sent_at) VALUES \
                 ('a', '1', '{}', 0, ?1), ('a', '2', '{}', 0, ?2), ('a', '3', '{}', 0, NULL)",
                [old_time, NOW],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO jobs (kind, class, target, state, created_at, finished_at) VALUES \
                 ('k', 'c', '{}', 'succeeded', 0, ?1), ('k', 'c', '{}', 'failed', 0, ?1)",
                [old_time],
            )
            .unwrap();

        database.prune_bookkeeping(NOW).unwrap();

        let count = |table: &str| -> i64 {
            database
                .connection()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap()
        };
        assert_eq!(count("notifications"), 2);
        assert_eq!(count("jobs"), 1);
        assert_eq!(count("head_samples"), 3);
    }
}

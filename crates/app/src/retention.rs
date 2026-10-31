//! The retention sweep (D-12). Files go before rows, so a crash leaves a row without a file,
//! which the next sweep repairs.

use crate::error::AcquireError;
use dalsegno_io::fragments::FragmentStore;
use dalsegno_store::database::Database;
use serde::Serialize;
use std::collections::HashSet;
use std::io::ErrorKind;
use std::time::{Duration, SystemTime};

const DAY_MS: i64 = 24 * 3600 * 1000;
const AUDIO_RETENTION_DAYS: i64 = 14;
const SWEEP_BATCH: u32 = 1_000;
/// A younger file may belong to a running fetch, which adds the row after the file.
const UNTRACKED_FILE_AGE: Duration = Duration::from_secs(3600);

/// What a sweep removed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SweepReport {
    /// Fragments past retention.
    pub expired: u64,
    /// Files that had no row.
    pub orphans: u64,
    /// Abandoned temporary files.
    pub temporary: u64,
    /// Rows whose file was missing.
    pub repaired: u64,
}

/// Removes what retention allows, then repairs differences between files and rows.
pub fn sweep(
    database: &mut Database,
    store: &FragmentStore,
    now: i64,
) -> Result<SweepReport, AcquireError> {
    let mut report = SweepReport::default();
    remove_expired(database, store, now, &mut report)?;
    reconcile(database, store, &mut report)?;
    database.prune_bookkeeping(now)?;
    Ok(report)
}

fn remove_expired(
    database: &mut Database,
    store: &FragmentStore,
    now: i64,
    report: &mut SweepReport,
) -> Result<(), AcquireError> {
    let cutoff = now - AUDIO_RETENTION_DAYS * DAY_MS;
    let mut after = None;
    loop {
        let batch = database.sweep_candidates(cutoff, now, after, SWEEP_BATCH)?;
        let Some(last) = batch.last() else {
            return Ok(());
        };
        after = Some(*last);
        for item in batch {
            store.remove(item.stream_id, item.itag, item.sq)?;
            database.delete_swept(&item)?;
            report.expired += 1;
        }
    }
}

fn reconcile(
    database: &mut Database,
    store: &FragmentStore,
    report: &mut SweepReport,
) -> Result<(), AcquireError> {
    let mut known = HashSet::new();
    for (stream_id, itag) in database.fragment_tracks()? {
        for row in database.fragments_in_range(stream_id, itag, 0, i64::MAX as u64)? {
            if store.contains(stream_id, itag, row.sq) {
                known.insert((stream_id, itag, row.sq));
            } else {
                database.delete_fragment(stream_id, itag, row.sq)?;
                report.repaired += 1;
            }
        }
    }
    let oldest = SystemTime::now()
        .checked_sub(UNTRACKED_FILE_AGE)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    for file in store.files()? {
        let is_untracked = match file.key {
            Some(key) => !known.contains(&key),
            None => file.is_temporary,
        };
        let unwanted = is_untracked && file.modified < oldest;
        if !unwanted {
            continue;
        }
        match std::fs::remove_file(&file.path) {
            Ok(()) if file.key.is_some() => report.orphans += 1,
            Ok(()) => report.temporary += 1,
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

//! Filling recorded gaps from the DVR, oldest first (FR-A6, FR-A11).

use crate::error::AcquireError;
use crate::fetch::{FetchRange, FetchReport, Fetcher, MAX_RANGE_FRAGMENTS, fetch_range, lock};
use crate::probe::read_head;
use dalsegno_io::ytdlp::Resolver;
use dalsegno_store::gaps::{Gap, GapStatus};
use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Backfill reaches this far unless asked otherwise.
pub const DEFAULT_HOURS: f64 = 24.0;
const MAX_HOURS: f64 = 120.0;
const SECONDS_PER_HOUR: f64 = 3600.0;

/// What a backfill achieved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct BackfillReport {
    /// Latest `sq` when it started.
    pub head_sq: u64,
    /// Fragments fetched.
    pub fetched: u64,
    /// Fragments that left the DVR, now recorded as lost.
    pub lost: u64,
    /// Fragments still to do, which stay in open gaps.
    pub failed: u64,
}

/// Records what is missing from the last `hours` of the stream, then fills open gaps.
pub async fn backfill<R: Resolver>(
    fetcher: Arc<Fetcher<R>>,
    fragment_len: f64,
    hours: f64,
) -> Result<BackfillReport, AcquireError> {
    if !(hours > 0.0 && hours <= MAX_HOURS) {
        return Err(AcquireError::InvalidRequest("hours must be from 0 to 120"));
    }
    let lease = fetcher.session.lease().await?;
    let head_sq = read_head(&fetcher.client, &lease.url, lease.family).await?;
    let wanted = ((hours * SECONDS_PER_HOUR / fragment_len).ceil() as u64).min(MAX_RANGE_FRAGMENTS);
    let first = head_sq.saturating_sub(wanted).max(1);
    record_missing(&fetcher, first, head_sq.saturating_sub(1))?;
    let gaps = {
        let database = lock(&fetcher.ledger.database)?;
        database.open_gaps(fetcher.ledger.stream_id, fetcher.ledger.itag)?
    };
    let mut report = BackfillReport {
        head_sq,
        ..BackfillReport::default()
    };
    for gap in gaps {
        fill(&fetcher, gap, &mut report).await?;
    }
    Ok(report)
}

fn record_missing<R>(fetcher: &Fetcher<R>, first: u64, last: u64) -> Result<(), AcquireError> {
    let ledger = &fetcher.ledger;
    let database = lock(&ledger.database)?;
    let stored: BTreeSet<u64> = database
        .fragments_in_range(ledger.stream_id, ledger.itag, first, last)?
        .iter()
        .map(|row| row.sq)
        .collect();
    let known = database.unfilled_gaps(ledger.stream_id, ledger.itag)?;
    let missing = (first..=last)
        .filter(|sq| !stored.contains(sq))
        .filter(|sq| !known.iter().any(|gap| (gap.0..=gap.1).contains(sq)));
    for (gap_first, gap_last) in ranges(missing) {
        database.add_gap(ledger.stream_id, ledger.itag, gap_first, gap_last)?;
    }
    Ok(())
}

async fn fill<R: Resolver>(
    fetcher: &Arc<Fetcher<R>>,
    gap: Gap,
    report: &mut BackfillReport,
) -> Result<(), AcquireError> {
    let mut total = FetchReport::default();
    let mut first = gap.sq_first;
    while first <= gap.sq_last {
        let last = gap.sq_last.min(first + MAX_RANGE_FRAGMENTS - 1);
        let range = FetchRange {
            stream_id: gap.stream_id,
            itag: gap.itag,
            first,
            last,
        };
        let part = fetch_range(Arc::clone(fetcher), range).await?;
        total.fetched += part.fetched;
        total.lost.extend(part.lost);
        total.failed.extend(part.failed);
        first = last + 1;
    }
    report.fetched += total.fetched;
    report.lost += total.lost.len() as u64;
    report.failed += total.failed.len() as u64;
    let database = lock(&fetcher.ledger.database)?;
    for (lost_first, lost_last) in ranges(total.lost.iter().copied()) {
        let id = database.add_gap(gap.stream_id, gap.itag, lost_first, lost_last)?;
        database.set_gap_status(id, GapStatus::Lost)?;
    }
    match (total.failed.first(), total.failed.last()) {
        (Some(first_failed), Some(last_failed)) => {
            database.shrink_gap(gap.id, *first_failed, *last_failed)?;
        }
        _ => database.set_gap_status(gap.id, GapStatus::Filled)?,
    }
    Ok(())
}

/// Groups ascending numbers into inclusive runs.
fn ranges(values: impl Iterator<Item = u64>) -> Vec<(u64, u64)> {
    let mut groups: Vec<(u64, u64)> = Vec::new();
    for value in values {
        match groups.last_mut() {
            Some((_, last)) if *last + 1 == value => *last = value,
            _ => groups.push((value, value)),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_contiguous_values() {
        assert_eq!(
            ranges([1, 2, 3, 7, 9, 10].into_iter()),
            [(1, 3), (7, 7), (9, 10)]
        );
        assert_eq!(ranges(std::iter::empty()), []);
    }
}

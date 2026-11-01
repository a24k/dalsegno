//! Following the head of a stream's audio, one fragment at a time.
//!
//! Following and backfill are independent. What following cannot reach becomes a gap, so one
//! fragment that cannot be fetched never holds it back.

use crate::backfill::record_missing;
use crate::error::AcquireError;
use crate::fetch::{FetchRange, Fetcher, fetch_range, lock};
use crate::probe::read_head;
use dalsegno_io::ytdlp::Resolver;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;

/// More than a short outage: after this, what was missed goes to backfill.
const MAX_BEHIND_SECONDS: f64 = 600.0;
const FAILURES_BEFORE_CHECK: u32 = 3;
const ENDED_CONFIRMATIONS: u32 = 3;
const ENDED_DELAY: Duration = Duration::from_secs(60);

/// How following reacts to trouble.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FollowPolicy {
    /// Fragments behind the head that make following jump to it.
    pub max_behind: u64,
    /// Failed fetches in a row before the head and the stream are checked.
    pub failures_before_check: u32,
    /// "Unavailable" answers in a row that mean the stream has ended.
    pub ended_confirmations: u32,
    /// Wait between those answers.
    pub ended_delay: Duration,
}

impl FollowPolicy {
    /// The defaults for a stream of this fragment length.
    pub fn for_fragment_length(fragment_len: f64) -> Self {
        Self {
            max_behind: (MAX_BEHIND_SECONDS / fragment_len).ceil() as u64,
            failures_before_check: FAILURES_BEFORE_CHECK,
            ended_confirmations: ENDED_CONFIRMATIONS,
            ended_delay: ENDED_DELAY,
        }
    }
}

/// Why following stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowEnd {
    /// It was told to.
    Stopped,
    /// The stream finished, for the reason given.
    Ended(String),
}

/// Follows the stream until it is told to stop, it ends, or it fails.
pub async fn follow<R: Resolver>(
    fetcher: Arc<Fetcher<R>>,
    policy: FollowPolicy,
    mut stop: watch::Receiver<bool>,
) -> Result<FollowEnd, AcquireError> {
    let mut next = start_at_head(&fetcher).await?;
    let mut failures = 0;
    while !*stop.borrow() {
        let range = FetchRange {
            stream_id: fetcher.ledger.stream_id,
            itag: fetcher.ledger.itag,
            first: next,
            last: next,
        };
        let fetched = tokio::select! {
            report = fetch_range(Arc::clone(&fetcher), range) => report?,
            _ = stop.changed() => break,
        };
        if fetched.is_complete() {
            next += 1;
            failures = 0;
            let database = lock(&fetcher.ledger.database)?;
            database.advance_follow(fetcher.ledger.stream_id, next)?;
            continue;
        }
        failures += 1;
        if failures < policy.failures_before_check {
            continue;
        }
        failures = 0;
        if let Some(reason) = confirm_ended(&fetcher, &policy).await? {
            return Ok(FollowEnd::Ended(reason));
        }
        next = jump_if_behind(&fetcher, &policy, next).await?;
    }
    Ok(FollowEnd::Stopped)
}

/// Finds where to start. What was missed since the last run becomes a gap.
async fn start_at_head<R: Resolver>(fetcher: &Fetcher<R>) -> Result<u64, AcquireError> {
    let lease = fetcher.session.lease().await?;
    let head = read_head(&fetcher.client, &lease.url, lease.family).await?;
    let ledger = &fetcher.ledger;
    let stored = lock(&ledger.database)?.follow_sq(ledger.stream_id)?;
    let next = match stored {
        Some(sq) if sq < head => {
            record_missing(fetcher, sq, head - 1)?;
            head
        }
        Some(sq) => sq,
        None => head,
    };
    lock(&ledger.database)?.set_follow_sq(ledger.stream_id, next)?;
    Ok(next)
}

/// Asks yt-dlp about the stream, as a head that stands still can be a stalled origin. The
/// answer is final, so it must repeat.
async fn confirm_ended<R: Resolver>(
    fetcher: &Fetcher<R>,
    policy: &FollowPolicy,
) -> Result<Option<String>, AcquireError> {
    for confirmation in 1..=policy.ended_confirmations {
        fetcher.session.refresh().await;
        match fetcher.session.lease().await {
            Ok(_) => return Ok(None),
            Err(AcquireError::Unavailable(reason)) => {
                if confirmation == policy.ended_confirmations {
                    return Ok(Some(reason));
                }
                tokio::time::sleep(policy.ended_delay).await;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(None)
}

async fn jump_if_behind<R: Resolver>(
    fetcher: &Fetcher<R>,
    policy: &FollowPolicy,
    next: u64,
) -> Result<u64, AcquireError> {
    let lease = fetcher.session.lease().await?;
    let head = read_head(&fetcher.client, &lease.url, lease.family).await?;
    if head.saturating_sub(next) <= policy.max_behind {
        return Ok(next);
    }
    record_missing(fetcher, next, head - 1)?;
    let ledger = &fetcher.ledger;
    lock(&ledger.database)?.set_follow_sq(ledger.stream_id, head)?;
    Ok(head)
}

//! Fetching a range of fragments. The file comes first and the row last, so a row always means
//! a complete file, and fragments with a row are skipped, so a rerun resumes (FR-A7).

use crate::error::AcquireError;
use crate::session::{Clock, Session};
use dalsegno_core::acquisition::{Outcome, ResponseFacts, backoff_delay, classify};
use dalsegno_core::container::{FragmentTiming, read_timing};
use dalsegno_io::fragments::FragmentStore;
use dalsegno_io::http::{FragmentClient, FragmentResponse};
use dalsegno_io::ytdlp::Resolver;
use dalsegno_store::database::Database;
use dalsegno_store::fragments::{FragmentRow, HeadSample};
use serde::Serialize;
use std::collections::{BTreeSet, VecDeque};
use std::fmt;
use std::sync::{Arc, MutexGuard};
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::{JoinSet, spawn_blocking};
use tokio::time::Instant;

/// More than 120 hours of fragments, even at a fragment length well under 5 s.
pub const MAX_RANGE_FRAGMENTS: u64 = 100_000;
const MILLIS_PER_SECOND: i64 = 1_000;
const HEAD_SAMPLE_INTERVAL_MS: i64 = 60_000;
const REQUEST_RATES: std::ops::RangeInclusive<f64> = 0.01..=1_000_000.0;

/// The inclusive range to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRange {
    /// Stream.
    pub stream_id: i64,
    /// Format.
    pub itag: u32,
    /// First `sq`.
    pub first: u64,
    /// Last `sq`.
    pub last: u64,
}

/// Request limits. Defaults stay under the measured ceiling of about 5 per second.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FetchLimits {
    /// Requests in flight.
    pub concurrency: usize,
    /// Request starts per second.
    pub requests_per_second: f64,
}

impl Default for FetchLimits {
    fn default() -> Self {
        Self {
            concurrency: 4,
            requests_per_second: 4.0,
        }
    }
}

/// How one fragment is retried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// First back-off delay.
    pub base_delay: Duration,
    /// Attempts per fragment before it is reported as failed.
    pub max_attempts: u32,
    /// Wait for a fragment past the head.
    pub not_yet_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            base_delay: Duration::from_secs(1),
            max_attempts: 6,
            not_yet_delay: Duration::from_secs(5),
        }
    }
}

/// What a range fetch achieved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FetchReport {
    /// Fragments that were already stored.
    pub already_stored: u64,
    /// Fragments fetched by this run.
    pub fetched: u64,
    /// Fragments that have left the DVR.
    pub lost: Vec<u64>,
    /// Fragments that kept failing. A rerun tries them again.
    pub failed: Vec<u64>,
}

impl FetchReport {
    /// Returns whether every fragment of the range is stored.
    pub fn is_complete(&self) -> bool {
        self.lost.is_empty() && self.failed.is_empty()
    }
}

/// The database, shared by tasks and locked for short operations only.
pub type SharedDatabase = Arc<std::sync::Mutex<Database>>;

/// Locks the database.
pub fn lock(database: &SharedDatabase) -> Result<MutexGuard<'_, Database>, AcquireError> {
    database.lock().map_err(|_| AcquireError::Task)
}

/// Where a fetch records what it stored.
#[derive(Debug, Clone)]
pub struct Ledger {
    /// Database.
    pub database: SharedDatabase,
    /// Stream.
    pub stream_id: i64,
    /// Format being fetched.
    pub itag: u32,
    /// Duration recorded for formats whose fragments do not carry one.
    pub fragment_len: f64,
}

/// The parts a range fetch works with.
pub struct Fetcher<R> {
    /// Lease handling for the stream and format being fetched.
    pub session: Session<R>,
    /// HTTP client.
    pub client: FragmentClient,
    /// Destination.
    pub store: FragmentStore,
    /// Request limits.
    pub limits: FetchLimits,
    /// Retry behaviour.
    pub retry: RetryPolicy,
    /// Database records.
    pub ledger: Ledger,
    /// Time source.
    pub clock: Clock,
}

impl<R> fmt::Debug for Fetcher<R> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Fetcher")
            .field("session", &self.session)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FragmentResult {
    Fetched,
    Lost,
    Failed,
}

#[derive(Debug)]
struct Pacer {
    interval: Duration,
    next: Mutex<Instant>,
}

impl Pacer {
    fn new(requests_per_second: f64) -> Self {
        Self {
            interval: Duration::from_secs_f64(1.0 / requests_per_second),
            next: Mutex::new(Instant::now()),
        }
    }

    async fn wait(&self) {
        let slot = {
            let mut next = self.next.lock().await;
            let slot = (*next).max(Instant::now());
            *next = slot + self.interval;
            slot
        };
        tokio::time::sleep_until(slot).await;
    }
}

#[derive(Debug)]
struct Run<R> {
    fetcher: Arc<Fetcher<R>>,
    range: FetchRange,
    last_head_sample: Mutex<i64>,
    pacer: Pacer,
    queue: Mutex<VecDeque<u64>>,
    report: Mutex<FetchReport>,
}

/// Fetches every fragment of `range` that is not stored yet, oldest first.
pub async fn fetch_range<R: Resolver>(
    fetcher: Arc<Fetcher<R>>,
    range: FetchRange,
) -> Result<FetchReport, AcquireError> {
    validate(&fetcher.limits, &range)?;
    let missing = missing_fragments(&fetcher.ledger.database, &range)?;
    let total = range.last - range.first + 1;
    let already_stored = total - missing.len() as u64;
    let concurrency = fetcher.limits.concurrency;
    let last_head_sample = lock(&fetcher.ledger.database)?
        .latest_head_sample(range.stream_id)?
        .map_or(i64::MIN, |sample| sample.observed_at);
    let run = Arc::new(Run {
        pacer: Pacer::new(fetcher.limits.requests_per_second),
        fetcher,
        range,
        last_head_sample: Mutex::new(last_head_sample),
        queue: Mutex::new(missing),
        report: Mutex::new(FetchReport {
            already_stored,
            ..FetchReport::default()
        }),
    });
    let mut workers = JoinSet::new();
    for _ in 0..concurrency {
        workers.spawn(Arc::clone(&run).work());
    }
    while let Some(joined) = workers.join_next().await {
        joined.map_err(|_| AcquireError::Task)??;
    }
    let report = run.report.lock().await.clone();
    Ok(sorted(report))
}

fn validate(limits: &FetchLimits, range: &FetchRange) -> Result<(), AcquireError> {
    if range.first > range.last {
        return Err(AcquireError::InvalidRequest("the range is reversed"));
    }
    if limits.concurrency == 0 {
        return Err(AcquireError::InvalidRequest("concurrency must be positive"));
    }
    if range.last - range.first >= MAX_RANGE_FRAGMENTS {
        return Err(AcquireError::InvalidRequest("the range exceeds the DVR"));
    }
    if !REQUEST_RATES.contains(&limits.requests_per_second) {
        return Err(AcquireError::InvalidRequest(
            "the request rate is out of range",
        ));
    }
    Ok(())
}

fn missing_fragments(
    database: &SharedDatabase,
    range: &FetchRange,
) -> Result<VecDeque<u64>, AcquireError> {
    let stored: BTreeSet<u64> = lock(database)?
        .fragments_in_range(range.stream_id, range.itag, range.first, range.last)?
        .iter()
        .map(|row| row.sq)
        .collect();
    Ok((range.first..=range.last)
        .filter(|sq| !stored.contains(sq))
        .collect())
}

fn sorted(report: FetchReport) -> FetchReport {
    let mut lost = report.lost;
    let mut failed = report.failed;
    lost.sort_unstable();
    failed.sort_unstable();
    FetchReport {
        lost,
        failed,
        ..report
    }
}

impl<R: Resolver> Run<R> {
    async fn work(self: Arc<Self>) -> Result<(), AcquireError> {
        loop {
            let Some(sq) = self.queue.lock().await.pop_front() else {
                return Ok(());
            };
            let result = self.fetch_fragment(sq).await?;
            let mut report = self.report.lock().await;
            match result {
                FragmentResult::Fetched => report.fetched += 1,
                FragmentResult::Lost => report.lost.push(sq),
                FragmentResult::Failed => report.failed.push(sq),
            }
        }
    }

    async fn fetch_fragment(&self, sq: u64) -> Result<FragmentResult, AcquireError> {
        let retry = self.fetcher.retry;
        let mut attempts = 0;
        loop {
            let lease = self.fetcher.session.lease().await?;
            self.pacer.wait().await;
            let response = self.fetcher.client.get(&lease.url, sq, lease.family).await;
            let (outcome, response) = match response {
                Ok(response) => (outcome_of(sq, &response), Some(response)),
                Err(error) => {
                    tracing::warn!(stream = self.range.stream_id, sq, %error, "request failed");
                    (Outcome::Backoff, None)
                }
            };
            let delay = match outcome {
                Outcome::Success => {
                    let response = response.ok_or(AcquireError::Task)?;
                    match read_timing(&response.body) {
                        Ok(timing) => {
                            self.record(sq, timing, response).await?;
                            self.fetcher.session.report_success(&lease).await;
                            return Ok(FragmentResult::Fetched);
                        }
                        Err(error) => {
                            let stream = self.range.stream_id;
                            tracing::warn!(stream, sq, %error, "unreadable fragment");
                            backoff_delay(retry.base_delay, attempts, fastrand::f64())
                        }
                    }
                }
                Outcome::Lost => return Ok(FragmentResult::Lost),
                Outcome::Unexpected(status) => {
                    tracing::warn!(
                        stream = self.range.stream_id,
                        sq,
                        status,
                        "unexpected status"
                    );
                    return Ok(FragmentResult::Failed);
                }
                Outcome::RefreshUrl => {
                    self.fetcher.session.report_refused(&lease).await?;
                    Duration::ZERO
                }
                Outcome::Empty => {
                    self.fetcher.session.report_empty(&lease).await?;
                    backoff_delay(retry.base_delay, attempts, fastrand::f64())
                }
                Outcome::NotYet => retry.not_yet_delay,
                Outcome::Backoff => backoff_delay(retry.base_delay, attempts, fastrand::f64()),
            };
            attempts += 1;
            if attempts >= retry.max_attempts {
                return Ok(FragmentResult::Failed);
            }
            tokio::time::sleep(delay).await;
        }
    }

    async fn record(
        &self,
        sq: u64,
        timing: FragmentTiming,
        response: FragmentResponse,
    ) -> Result<(), AcquireError> {
        let now = (self.fetcher.clock)() as i64 * MILLIS_PER_SECOND;
        let head = self.head_sample(&response, now).await;
        let fetcher = Arc::clone(&self.fetcher);
        let range = self.range.clone();
        let ledger_len = fetcher.ledger.fragment_len;
        let row = FragmentRow {
            stream_id: range.stream_id,
            itag: range.itag,
            sq,
            stream_time: timing.start_time,
            duration: timing.duration.unwrap_or(ledger_len),
            size: response.body.len() as u64,
            fetched_at: now,
        };
        spawn_blocking(move || {
            let store = &fetcher.store;
            store.write(range.stream_id, range.itag, sq, &response.body)?;
            let database = lock(&fetcher.ledger.database)?;
            database.put_fragment(&row)?;
            database.set_last_fetch(range.stream_id, now)?;
            if let Some(sample) = head {
                database.put_head_sample(&sample)?;
            }
            Ok::<_, AcquireError>(())
        })
        .await
        .map_err(|_| AcquireError::Task)?
    }

    /// Returns a sample from these headers, at most once a minute.
    async fn head_sample(&self, response: &FragmentResponse, now: i64) -> Option<HeadSample> {
        let observed_at = response
            .walltime_ms
            .and_then(|value| i64::try_from(value).ok())
            .unwrap_or(now);
        let mut last = self.last_head_sample.lock().await;
        if observed_at.saturating_sub(*last) < HEAD_SAMPLE_INTERVAL_MS {
            return None;
        }
        let sample = HeadSample {
            stream_id: self.range.stream_id,
            observed_at,
            head_sq: response.head_seqnum?,
            head_time: response.head_time_ms? as f64 / MILLIS_PER_SECOND as f64,
        };
        *last = observed_at;
        Some(sample)
    }
}

fn outcome_of(sq: u64, response: &FragmentResponse) -> Outcome {
    classify(
        sq,
        ResponseFacts {
            status: response.status,
            body_len: response.body.len(),
            head_seqnum: response.head_seqnum,
            sequence_num: response.sequence_num,
        },
    )
}

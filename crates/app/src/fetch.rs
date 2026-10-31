//! Fetching a range of fragments. Stored fragments are skipped, so a rerun resumes (FR-A7).

use crate::error::AcquireError;
use crate::session::Session;
use dalsegno_core::acquisition::{Outcome, ResponseFacts, backoff_delay, classify};
use dalsegno_core::stream::VideoId;
use dalsegno_io::fragments::FragmentStore;
use dalsegno_io::http::{FragmentClient, FragmentResponse};
use dalsegno_io::ytdlp::Resolver;
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::{JoinSet, spawn_blocking};
use tokio::time::Instant;

/// More than 120 hours of fragments, even at a fragment length well under 5 s.
const MAX_RANGE_FRAGMENTS: u64 = 100_000;
const REQUEST_RATES: std::ops::RangeInclusive<f64> = 0.01..=1_000_000.0;

/// The inclusive range to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRange {
    /// Stream.
    pub video: VideoId,
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

/// The parts a range fetch works with.
#[derive(Debug)]
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
    fetcher: Fetcher<R>,
    range: FetchRange,
    pacer: Pacer,
    queue: Mutex<VecDeque<u64>>,
    report: Mutex<FetchReport>,
}

/// Fetches every fragment of `range` that is not stored yet, oldest first.
pub async fn fetch_range<R: Resolver>(
    fetcher: Fetcher<R>,
    range: FetchRange,
) -> Result<FetchReport, AcquireError> {
    validate(&fetcher.limits, &range)?;
    let store = fetcher.store.clone();
    let wanted = range.clone();
    let missing = spawn_blocking(move || missing_fragments(&store, &wanted))
        .await
        .map_err(|_| AcquireError::Task)?;
    let total = range.last - range.first + 1;
    let already_stored = total - missing.len() as u64;
    let concurrency = fetcher.limits.concurrency;
    let run = Arc::new(Run {
        pacer: Pacer::new(fetcher.limits.requests_per_second),
        fetcher,
        range,
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

fn missing_fragments(store: &FragmentStore, range: &FetchRange) -> VecDeque<u64> {
    (range.first..=range.last)
        .filter(|sq| !store.contains(&range.video, range.itag, *sq))
        .collect()
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
            let (outcome, body) = match response {
                Ok(response) => split(sq, response),
                Err(error) => {
                    tracing::warn!(stream = %self.range.video, sq, %error, "fragment request failed");
                    (Outcome::Backoff, Vec::new())
                }
            };
            let delay = match outcome {
                Outcome::Success => {
                    self.store(sq, body).await?;
                    self.fetcher.session.report_success(&lease).await;
                    return Ok(FragmentResult::Fetched);
                }
                Outcome::Lost => return Ok(FragmentResult::Lost),
                Outcome::Unexpected(status) => {
                    tracing::warn!(stream = %self.range.video, sq, status, "unexpected status");
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

    async fn store(&self, sq: u64, body: Vec<u8>) -> Result<(), AcquireError> {
        let store = self.fetcher.store.clone();
        let video = self.range.video.clone();
        let itag = self.range.itag;
        spawn_blocking(move || store.write(&video, itag, sq, &body))
            .await
            .map_err(|_| AcquireError::Task)??;
        Ok(())
    }
}

fn split(sq: u64, response: FragmentResponse) -> (Outcome, Vec<u8>) {
    let facts = ResponseFacts {
        status: response.status,
        body_len: response.body.len(),
        head_seqnum: response.head_seqnum,
        sequence_num: response.sequence_num,
    };
    (classify(sq, facts), response.body)
}

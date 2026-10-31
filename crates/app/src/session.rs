//! The URL lease of one stream and format, and the failure ladder around it.
//!
//! Leases live in memory only, because they contain the client IP.

use crate::error::AcquireError;
use crate::family::{SharedFamily, SwitchReason, Transition};
use dalsegno_core::acquisition::{backoff_delay, lease_is_fresh, parse_expire};
use dalsegno_core::redact::Redacted;
use dalsegno_core::stream::{IpFamily, VideoId};
use dalsegno_io::ytdlp::{ResolveError, Resolver};
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;

const RESOLVE_ATTEMPTS: u32 = 4;
/// A URL that is short-lived from the start is still used, instead of resolving on every request.
const MIN_LEASE_SECONDS: u64 = 300;
/// Empty responses in a row that count as one refusal.
const EMPTIES_PER_STRIKE: u32 = 3;
/// The second refusal in a row moves past a URL refresh to the family switch.
const STRIKES_BEFORE_SWITCH: u32 = 2;

/// Returns the current time in Unix seconds.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// A clock reading the system time.
pub fn system_clock() -> Clock {
    Arc::new(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs())
    })
}

/// A URL handed to a fetch. The generation ties later reports to this URL.
#[derive(Debug, Clone)]
pub struct Lease {
    /// Fragment URL.
    pub url: Redacted<String>,
    /// Family the URL was resolved on.
    pub family: IpFamily,
    generation: u64,
}

#[derive(Debug)]
struct Held {
    url: Redacted<String>,
    family: IpFamily,
    expire: Option<u64>,
    resolved_at: u64,
}

impl Held {
    fn is_usable(&self, family: IpFamily, now: u64) -> bool {
        let is_fresh = self.expire.is_none_or(|expire| lease_is_fresh(expire, now));
        let is_recent = now < self.resolved_at.saturating_add(MIN_LEASE_SECONDS);
        self.family == family && (is_fresh || is_recent)
    }
}

#[derive(Debug, Default)]
struct State {
    held: Option<Held>,
    generation: u64,
    strikes: u32,
    empties: u32,
}

/// Lease handling for one stream and format.
pub struct Session<R> {
    resolver: Arc<R>,
    video: VideoId,
    itag: u32,
    family: SharedFamily,
    clock: Clock,
    retry_base: Duration,
    state: Mutex<State>,
}

impl<R> fmt::Debug for Session<R> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Session")
            .field("video", &self.video)
            .field("itag", &self.itag)
            .finish_non_exhaustive()
    }
}

impl<R: Resolver> Session<R> {
    /// Creates a session that holds no lease yet.
    pub fn new(
        resolver: Arc<R>,
        video: VideoId,
        itag: u32,
        family: SharedFamily,
        clock: Clock,
        retry_base: Duration,
    ) -> Self {
        Self {
            resolver,
            video,
            itag,
            family,
            clock,
            retry_base,
            state: Mutex::default(),
        }
    }

    /// Returns a usable lease, resolving one if needed.
    ///
    /// The state stays locked during resolution, so concurrent fetches share one yt-dlp call.
    pub async fn lease(&self) -> Result<Lease, AcquireError> {
        let mut state = self.state.lock().await;
        self.recover_family().await;
        let family = self.family.lock().await.active();
        let now = (self.clock)();
        if let Some(held) = &state.held
            && held.is_usable(family, now)
        {
            return Ok(Lease {
                url: held.url.clone(),
                family: held.family,
                generation: state.generation,
            });
        }
        let held = self.resolve().await?;
        state.generation += 1;
        let lease = Lease {
            url: held.url.clone(),
            family: held.family,
            generation: state.generation,
        };
        state.held = Some(held);
        Ok(lease)
    }

    /// Reports a refusal: refresh the URL, then switch family, then give up (FR-A5).
    pub async fn report_refused(&self, lease: &Lease) -> Result<(), AcquireError> {
        let mut state = self.state.lock().await;
        if lease.generation != state.generation {
            return Ok(());
        }
        self.strike(&mut state).await
    }

    /// Reports an empty 200, which counts towards a refusal once it repeats.
    pub async fn report_empty(&self, lease: &Lease) -> Result<(), AcquireError> {
        let mut state = self.state.lock().await;
        if lease.generation != state.generation {
            return Ok(());
        }
        state.empties += 1;
        if state.empties < EMPTIES_PER_STRIKE {
            return Ok(());
        }
        state.empties = 0;
        self.strike(&mut state).await
    }

    /// Reports a served fragment, which clears the failure ladder.
    pub async fn report_success(&self, lease: &Lease) {
        let mut state = self.state.lock().await;
        if lease.generation == state.generation {
            state.strikes = 0;
            state.empties = 0;
        }
    }

    async fn strike(&self, state: &mut State) -> Result<(), AcquireError> {
        state.strikes += 1;
        state.held = None;
        state.generation += 1;
        if state.strikes >= STRIKES_BEFORE_SWITCH {
            self.switch_family(SwitchReason::PersistentRefusal).await?;
        }
        Ok(())
    }

    async fn switch_family(&self, reason: SwitchReason) -> Result<(), AcquireError> {
        let mut family = self.family.lock().await;
        match family.on_origin_failure((self.clock)(), reason) {
            Transition::Switched(to) => {
                tracing::warn!(stream = %self.video, itag = self.itag, %to, ?reason, "switched IP family");
                Ok(())
            }
            Transition::Exhausted => Err(AcquireError::FamilyExhausted(family.active())),
        }
    }

    async fn resolve(&self) -> Result<Held, AcquireError> {
        let mut failures = 0;
        loop {
            let family = self.family.lock().await.active();
            match self.resolver.resolve(&self.video, self.itag, family).await {
                Ok(url) => {
                    return Ok(Held {
                        expire: parse_expire(url.expose()),
                        url,
                        family,
                        resolved_at: (self.clock)(),
                    });
                }
                Err(ResolveError::BotCheck) => self.switch_family(SwitchReason::BotCheck).await?,
                Err(ResolveError::Unavailable(message)) => {
                    return Err(AcquireError::Unavailable(message));
                }
                Err(ResolveError::Failed(message)) => {
                    failures += 1;
                    if failures >= RESOLVE_ATTEMPTS {
                        return Err(AcquireError::Resolve(message));
                    }
                    let delay = backoff_delay(self.retry_base, failures, fastrand::f64());
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    async fn recover_family(&self) {
        let now = (self.clock)();
        let mut family = self.family.lock().await;
        if !family.probe_due(now) {
            return;
        }
        let preferred = family.preferred();
        let result = self
            .resolver
            .resolve(&self.video, self.itag, preferred)
            .await;
        family.on_probe_result(now, result.is_ok());
    }
}

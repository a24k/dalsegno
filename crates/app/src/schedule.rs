//! The one budget for requests to the origin, shared by every stream and job (FR-A8).
//!
//! A request waits for a slot: fewer than `concurrency` in flight, and a start at least one
//! interval after the last. Waiting requests go in priority order, so following never queues
//! behind backfill. When the origin asks to back off, the rate is halved for a while, because
//! its limits count requests rather than bytes.

use crate::fetch::FetchLimits;
use dalsegno_core::acquisition::{Outcome, ResponseFacts, classify};
use dalsegno_core::redact::Redacted;
use dalsegno_core::stream::IpFamily;
use dalsegno_io::http::{FragmentClient, FragmentResponse, TransportError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

/// How long the rate stays halved after the origin asks to back off.
const SLOWDOWN: Duration = Duration::from_secs(600);
const SLOWDOWN_FACTOR: u32 = 2;
/// Used only if limits that were never validated reach the scheduler.
const FALLBACK_INTERVAL: Duration = Duration::from_secs(1);

/// Who is asking, most urgent first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// Live following, which loses what it misses once the DVR moves on.
    Follow,
    /// Ranges a person or an export is waiting for, and probes.
    Prefetch,
    /// Filling gaps, which can always wait.
    Backfill,
}

const PRIORITIES: usize = 3;

impl Priority {
    fn index(self) -> usize {
        match self {
            Self::Follow => 0,
            Self::Prefetch => 1,
            Self::Backfill => 2,
        }
    }
}

/// The shared budget.
#[derive(Debug)]
pub struct Scheduler {
    limits: FetchLimits,
    state: Mutex<State>,
    changed: Notify,
}

#[derive(Debug)]
struct State {
    in_flight: usize,
    next_start: Instant,
    waiting: [usize; PRIORITIES],
    slow_until: Option<Instant>,
}

/// A slot in flight. Dropping it frees the slot.
#[derive(Debug)]
pub struct Permit {
    scheduler: Arc<Scheduler>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        self.scheduler.state().in_flight -= 1;
        self.scheduler.changed.notify_waiters();
    }
}

/// A place in the queue, given up when the request stops waiting for any reason.
struct Ticket<'a> {
    scheduler: &'a Scheduler,
    priority: Priority,
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        self.scheduler.state().waiting[self.priority.index()] -= 1;
        self.scheduler.changed.notify_waiters();
    }
}

/// What a waiting request needs before it can start.
enum Wait {
    /// A slot, or for a more urgent request to go first.
    Turn,
    /// Only the clock.
    Until(Instant),
}

impl Scheduler {
    /// A budget with these limits, which [`FetchLimits::validate`] has accepted.
    pub fn new(limits: FetchLimits) -> Arc<Self> {
        Arc::new(Self {
            limits,
            state: Mutex::new(State {
                in_flight: 0,
                next_start: Instant::now(),
                waiting: [0; PRIORITIES],
                slow_until: None,
            }),
            changed: Notify::new(),
        })
    }

    /// The configured limits.
    pub fn limits(&self) -> FetchLimits {
        self.limits
    }

    /// Waits for a slot.
    pub async fn acquire(self: &Arc<Self>, priority: Priority) -> Permit {
        self.state().waiting[priority.index()] += 1;
        let _ticket = Ticket {
            scheduler: self,
            priority,
        };
        loop {
            // Registered before the state is read, so a change in between still wakes it.
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            match self.try_start(priority) {
                Ok(()) => {
                    return Permit {
                        scheduler: Arc::clone(self),
                    };
                }
                Err(Wait::Turn) => changed.await,
                Err(Wait::Until(at)) => {
                    tokio::select! {
                        () = tokio::time::sleep_until(at) => {}
                        () = changed => {}
                    }
                }
            }
        }
    }

    /// Halves the rate for a while, as the origin asked to back off.
    pub fn report_backoff(&self) {
        let now = Instant::now();
        let mut state = self.state();
        if state.slow_until.is_none_or(|until| until <= now) {
            tracing::warn!(
                minutes = SLOWDOWN.as_secs() / 60,
                "the origin asked to back off, so the request rate is halved"
            );
        }
        state.slow_until = Some(now + SLOWDOWN);
    }

    fn try_start(&self, priority: Priority) -> Result<(), Wait> {
        let now = Instant::now();
        let mut state = self.state();
        let is_outranked = state.waiting[..priority.index()]
            .iter()
            .any(|&count| count > 0);
        if is_outranked || state.in_flight >= self.limits.concurrency {
            return Err(Wait::Turn);
        }
        if now < state.next_start {
            return Err(Wait::Until(state.next_start));
        }
        let is_slowed = state.slow_until.is_some_and(|until| now < until);
        state.in_flight += 1;
        state.next_start = now + self.interval(is_slowed);
        Ok(())
    }

    fn interval(&self, is_slowed: bool) -> Duration {
        let base = Duration::try_from_secs_f64(1.0 / self.limits.requests_per_second)
            .unwrap_or(FALLBACK_INTERVAL);
        if is_slowed {
            base * SLOWDOWN_FACTOR
        } else {
            base
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // Every update completes before it can panic, so a poisoned state is still whole.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Requests to the origin on behalf of one kind of work. Every request takes a slot first.
#[derive(Debug, Clone)]
pub struct Requests {
    client: FragmentClient,
    scheduler: Arc<Scheduler>,
    priority: Priority,
}

impl Requests {
    /// Requests at `priority` within `scheduler`'s budget.
    pub fn new(client: FragmentClient, scheduler: Arc<Scheduler>, priority: Priority) -> Self {
        Self {
            client,
            scheduler,
            priority,
        }
    }

    /// The same budget, at another priority.
    pub fn at(&self, priority: Priority) -> Self {
        Self {
            priority,
            ..self.clone()
        }
    }

    /// The limits of the shared budget.
    pub fn limits(&self) -> FetchLimits {
        self.scheduler.limits()
    }

    /// Requests one fragment once its turn comes.
    pub async fn get(
        &self,
        url: &Redacted<String>,
        sq: u64,
        family: IpFamily,
    ) -> Result<FragmentResponse, TransportError> {
        let _permit = self.scheduler.acquire(self.priority).await;
        let response = self.client.get(url, sq, family).await?;
        let facts = ResponseFacts {
            status: response.status,
            body_len: response.body.len(),
            head_seqnum: response.head_seqnum,
            sequence_num: response.sequence_num,
        };
        if classify(sq, facts) == Outcome::Backoff {
            self.scheduler.report_backoff();
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::time::Instant;

    fn scheduler(concurrency: usize, requests_per_second: f64) -> Arc<Scheduler> {
        Scheduler::new(FetchLimits {
            concurrency,
            requests_per_second,
        })
    }

    /// Lets spawned tasks run until they wait.
    async fn settle() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn starts_are_spaced_by_the_rate() {
        let scheduler = scheduler(4, 10.0);
        let started = Instant::now();

        for _ in 0..4 {
            drop(scheduler.acquire(Priority::Backfill).await);
        }

        assert_eq!(started.elapsed(), Duration::from_millis(300));
    }

    #[tokio::test(start_paused = true)]
    async fn no_more_than_the_concurrency_are_in_flight() {
        let scheduler = scheduler(2, 1_000.0);
        let first = scheduler.acquire(Priority::Backfill).await;
        let _second = scheduler.acquire(Priority::Backfill).await;
        let third = tokio::spawn({
            let scheduler = Arc::clone(&scheduler);
            async move { scheduler.acquire(Priority::Follow).await }
        });

        settle().await;
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(!third.is_finished());

        drop(first);
        third.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn following_goes_before_backfill_that_waited_longer() {
        let scheduler = scheduler(1, 1_000.0);
        let order = Arc::new(Mutex::new(Vec::new()));
        let held = scheduler.acquire(Priority::Backfill).await;
        let mut waiting = Vec::new();
        for priority in [Priority::Backfill, Priority::Prefetch, Priority::Follow] {
            let (scheduler, order) = (Arc::clone(&scheduler), Arc::clone(&order));
            waiting.push(tokio::spawn(async move {
                let _permit = scheduler.acquire(priority).await;
                order.lock().unwrap().push(priority);
            }));
            settle().await;
        }

        drop(held);
        for task in waiting {
            task.await.unwrap();
        }

        assert_eq!(
            *order.lock().unwrap(),
            [Priority::Follow, Priority::Prefetch, Priority::Backfill]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_that_stops_waiting_does_not_hold_back_the_others() {
        let scheduler = scheduler(1, 1_000.0);
        let held = scheduler.acquire(Priority::Backfill).await;
        let follow = tokio::spawn({
            let scheduler = Arc::clone(&scheduler);
            async move { scheduler.acquire(Priority::Follow).await }
        });
        settle().await;
        let backfill = tokio::spawn({
            let scheduler = Arc::clone(&scheduler);
            async move { scheduler.acquire(Priority::Backfill).await }
        });
        settle().await;

        follow.abort();
        let _ = follow.await;
        drop(held);

        let started = tokio::time::timeout(Duration::from_secs(60), backfill).await;
        assert!(
            started.is_ok(),
            "the abandoned request still holds back backfill"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn backing_off_halves_the_rate_for_ten_minutes() {
        let scheduler = scheduler(4, 10.0);
        drop(scheduler.acquire(Priority::Follow).await);
        scheduler.report_backoff();

        // 100 ms were already due before the back-off, then 200 ms at the halved rate.
        let started = Instant::now();
        drop(scheduler.acquire(Priority::Follow).await);
        drop(scheduler.acquire(Priority::Follow).await);
        let slowed = started.elapsed();
        tokio::time::sleep(Duration::from_secs(600)).await;
        let started = Instant::now();
        drop(scheduler.acquire(Priority::Follow).await);
        drop(scheduler.acquire(Priority::Follow).await);
        let recovered = started.elapsed();

        assert_eq!(slowed, Duration::from_millis(300));
        assert_eq!(recovered, Duration::from_millis(100));
    }
}

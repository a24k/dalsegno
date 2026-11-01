//! Following the head against a mock origin.

mod common;

use common::*;
use dalsegno_app::follow::{FollowEnd, FollowPolicy, follow};
use dalsegno_store::streams::StreamState;
use tokio::sync::watch;

const HEAD: u64 = 100;
const WAIT: Duration = Duration::from_secs(5);

fn policy() -> FollowPolicy {
    FollowPolicy {
        max_behind: 50,
        failures_before_check: 1,
        ended_confirmations: 2,
        ended_delay: Duration::ZERO,
    }
}

async fn serve_head(harness: &Harness, head: u64) {
    Mock::given(query_param("sq", "1"))
        .respond_with(ResponseTemplate::new(404).insert_header("X-Head-Seqnum", head.to_string()))
        .mount(&harness.server)
        .await;
}

fn fetcher(harness: &Harness) -> Arc<Fetcher<FakeResolver>> {
    let mut fetcher = harness.fetcher(FetchLimits {
        concurrency: 1,
        requests_per_second: 10_000.0,
    });
    fetcher.retry = RetryPolicy {
        base_delay: Duration::from_millis(1),
        max_attempts: 2,
        not_yet_delay: Duration::from_millis(1),
    };
    Arc::new(fetcher)
}

fn set_state(harness: &Harness, state: StreamState) {
    let database = harness.database.lock().unwrap();
    database
        .set_stream_state(harness.stream_id, state, None)
        .unwrap();
}

fn follow_sq(harness: &Harness) -> Option<u64> {
    let database = harness.database.lock().unwrap();
    database.follow_sq(harness.stream_id).unwrap()
}

fn state(harness: &Harness) -> StreamState {
    let database = harness.database.lock().unwrap();
    database.stream_state(harness.stream_id).unwrap().unwrap().0
}

fn gaps(harness: &Harness) -> Vec<(u64, u64)> {
    let database = harness.database.lock().unwrap();
    database.unfilled_gaps(harness.stream_id, ITAG).unwrap()
}

async fn until(what: impl Fn() -> bool) {
    let start = Instant::now();
    while !what() {
        assert!(start.elapsed() < WAIT, "timed out");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

#[tokio::test]
async fn following_starts_at_the_head_and_advances() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    set_state(&harness, StreamState::Backfilling);
    serve_head(&harness, HEAD).await;
    serve_fragment(&harness.server, 100).await;
    serve_fragment(&harness.server, 101).await;
    let (stop, stopped) = watch::channel(false);
    let running = tokio::spawn(follow(fetcher(&harness), policy(), stopped));

    until(|| follow_sq(&harness) == Some(102)).await;
    stop.send(true).unwrap();

    assert_eq!(running.await.unwrap().unwrap(), FollowEnd::Stopped);
    assert_eq!(harness.rows(100, 101).len(), 2);
    assert_eq!(gaps(&harness), []);
}

#[tokio::test]
async fn the_first_stored_fragment_ends_backfilling() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    set_state(&harness, StreamState::Backfilling);
    serve_head(&harness, HEAD).await;
    serve_fragment(&harness.server, 100).await;
    let (stop, stopped) = watch::channel(false);
    let running = tokio::spawn(follow(fetcher(&harness), policy(), stopped));

    until(|| state(&harness) == StreamState::Recording).await;
    stop.send(true).unwrap();

    running.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_restart_records_what_it_missed_as_a_gap() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    harness
        .database
        .lock()
        .unwrap()
        .set_follow_sq(harness.stream_id, 90)
        .unwrap();
    serve_head(&harness, HEAD).await;
    serve_fragment(&harness.server, 100).await;
    let (stop, stopped) = watch::channel(false);
    let running = tokio::spawn(follow(fetcher(&harness), policy(), stopped));

    until(|| follow_sq(&harness) == Some(101)).await;
    stop.send(true).unwrap();

    running.await.unwrap().unwrap();
    assert_eq!(gaps(&harness), [(90, 99)]);
}

#[tokio::test]
async fn a_restart_at_the_head_records_nothing() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    harness
        .database
        .lock()
        .unwrap()
        .set_follow_sq(harness.stream_id, HEAD)
        .unwrap();
    serve_head(&harness, HEAD).await;
    serve_fragment(&harness.server, 100).await;
    let (stop, stopped) = watch::channel(false);
    let running = tokio::spawn(follow(fetcher(&harness), policy(), stopped));

    until(|| follow_sq(&harness) == Some(101)).await;
    stop.send(true).unwrap();

    running.await.unwrap().unwrap();
    assert_eq!(gaps(&harness), []);
}

#[tokio::test]
async fn falling_far_behind_records_a_gap_and_jumps_to_the_head() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    Mock::given(query_param("sq", "1"))
        .respond_with(ResponseTemplate::new(404).insert_header("X-Head-Seqnum", "100"))
        .up_to_n_times(1)
        .mount(&harness.server)
        .await;
    serve_head(&harness, 500).await;
    Mock::given(query_param("sq", "100"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&harness.server)
        .await;
    serve_fragment(&harness.server, 500).await;
    let (stop, stopped) = watch::channel(false);
    let running = tokio::spawn(follow(fetcher(&harness), policy(), stopped));

    until(|| follow_sq(&harness) == Some(501)).await;
    stop.send(true).unwrap();

    running.await.unwrap().unwrap();
    assert_eq!(gaps(&harness), [(100, 499)]);
}

#[tokio::test]
async fn a_head_that_stops_moving_does_not_end_a_live_stream() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    serve_head(&harness, HEAD).await;
    Mock::given(query_param("sq", "100"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&harness.server)
        .await;
    let (stop, stopped) = watch::channel(false);
    let running = tokio::spawn(follow(fetcher(&harness), policy(), stopped));

    until(|| harness.resolver.calls() >= 3).await;
    stop.send(true).unwrap();

    assert_eq!(running.await.unwrap().unwrap(), FollowEnd::Stopped);
}

#[tokio::test]
async fn a_stream_yt_dlp_keeps_calling_unavailable_has_ended() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    serve_head(&harness, HEAD).await;
    Mock::given(query_param("sq", "100"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&harness.server)
        .await;
    harness.fail_resolves_after_first(ResolveError::Unavailable("has ended".to_string()), 2);
    let (_stop, stopped) = watch::channel(false);

    let end = follow(fetcher(&harness), policy(), stopped).await.unwrap();

    assert!(matches!(end, FollowEnd::Ended(_)), "{end:?}");
}

#[tokio::test]
async fn one_unavailable_answer_does_not_end_a_stream() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    serve_head(&harness, HEAD).await;
    Mock::given(query_param("sq", "100"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&harness.server)
        .await;
    serve_fragment(&harness.server, 100).await;
    harness.fail_resolves_after_first(ResolveError::Unavailable("has ended".to_string()), 1);
    let (stop, stopped) = watch::channel(false);
    let running = tokio::spawn(follow(fetcher(&harness), policy(), stopped));

    until(|| follow_sq(&harness) == Some(101)).await;
    stop.send(true).unwrap();

    assert_eq!(running.await.unwrap().unwrap(), FollowEnd::Stopped);
}

#[tokio::test]
async fn a_failure_with_no_way_forward_reaches_the_caller() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    harness
        .resolver
        .errors
        .lock()
        .unwrap()
        .extend((0..4).map(|_| ResolveError::Failed("broken".to_string())));
    let (_stop, stopped) = watch::channel(false);

    assert!(follow(fetcher(&harness), policy(), stopped).await.is_err());
}

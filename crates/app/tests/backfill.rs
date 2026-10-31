//! Backfill against a mock origin.

mod common;

use common::*;
use dalsegno_app::backfill::backfill;

const HEAD: u64 = 100;
/// Three 5-second fragments, so the window is `sq` 97 to 99.
const HOURS: f64 = 15.0 / 3600.0;

async fn harness() -> Harness {
    let harness = Harness::new(IpPolicy::V4Only).await;
    Mock::given(query_param("sq", "1"))
        .respond_with(ResponseTemplate::new(404).insert_header("X-Head-Seqnum", "100"))
        .mount(&harness.server)
        .await;
    harness
}

async fn run(
    harness: &Harness,
    hours: f64,
) -> Result<dalsegno_app::backfill::BackfillReport, AcquireError> {
    let limits = FetchLimits {
        concurrency: 1,
        requests_per_second: 10_000.0,
    };
    backfill(Arc::new(harness.fetcher(limits)), 5.0, hours).await
}

fn open_gaps(harness: &Harness) -> Vec<(u64, u64)> {
    harness
        .database
        .lock()
        .unwrap()
        .open_gaps(harness.stream_id, ITAG)
        .unwrap()
        .iter()
        .map(|gap| (gap.sq_first, gap.sq_last))
        .collect()
}

fn gap_statuses(harness: &Harness) -> Vec<(u64, u64, String)> {
    let database = harness.database.lock().unwrap();
    let mut statement = database
        .connection()
        .prepare("SELECT sq_first, sq_last, status FROM gaps ORDER BY id")
        .unwrap();
    statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, i64>(1)? as u64,
                row.get(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[tokio::test]
async fn fills_the_window_behind_the_head() {
    let harness = harness().await;
    for sq in 97..=99 {
        serve_fragment(&harness.server, sq).await;
    }

    let report = run(&harness, HOURS).await.unwrap();

    assert_eq!((report.head_sq, report.fetched), (HEAD, 3));
    assert_eq!(harness.rows(97, 99).len(), 3);
    assert_eq!(gap_statuses(&harness), [(97, 99, "filled".to_string())]);
}

#[tokio::test]
async fn only_missing_fragments_become_gaps() {
    let harness = harness().await;
    harness.add_row(98);
    serve_fragment(&harness.server, 97).await;
    serve_fragment(&harness.server, 99).await;

    let report = run(&harness, HOURS).await.unwrap();

    assert_eq!(report.fetched, 2);
    assert_eq!(
        gap_statuses(&harness),
        [
            (97, 97, "filled".to_string()),
            (99, 99, "filled".to_string())
        ]
    );
}

#[tokio::test]
async fn fragments_gone_from_the_dvr_are_recorded_and_not_requested_again() {
    let harness = harness().await;
    Mock::given(query_param("sq", "97"))
        .respond_with(ResponseTemplate::new(404).insert_header("X-Head-Seqnum", "100"))
        .expect(1)
        .mount(&harness.server)
        .await;
    serve_fragment(&harness.server, 98).await;
    serve_fragment(&harness.server, 99).await;

    let report = run(&harness, HOURS).await.unwrap();
    let again = run(&harness, HOURS).await.unwrap();

    assert_eq!((report.fetched, report.lost), (2, 1));
    assert_eq!((again.fetched, again.lost), (0, 0));
    assert!(gap_statuses(&harness).contains(&(97, 97, "lost".to_string())));
    assert_eq!(open_gaps(&harness), []);
}

#[tokio::test]
async fn fragments_that_keep_failing_stay_open_for_the_next_run() {
    let harness = harness().await;
    Mock::given(query_param("sq", "98"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(4)
        .mount(&harness.server)
        .await;
    for sq in 97..=99 {
        serve_fragment(&harness.server, sq).await;
    }

    let first = run(&harness, HOURS).await.unwrap();
    assert_eq!((first.fetched, first.failed), (2, 1));
    assert_eq!(open_gaps(&harness), [(98, 98)]);

    let second = run(&harness, HOURS).await.unwrap();
    assert_eq!((second.fetched, second.failed), (1, 0));
    assert_eq!(open_gaps(&harness), []);
}

#[tokio::test]
async fn an_unreasonable_window_is_rejected() {
    let harness = harness().await;

    for hours in [0.0, -1.0, 121.0, f64::NAN] {
        assert!(matches!(
            run(&harness, hours).await,
            Err(AcquireError::InvalidRequest(_))
        ));
    }
    assert_eq!(harness.resolver.calls(), 0);
}

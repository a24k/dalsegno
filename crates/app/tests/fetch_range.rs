//! Range fetches against a mock origin: the failures listed in the M1 exit criteria.

mod common;

use common::*;

#[tokio::test]
async fn fetches_a_range_and_joins_the_bytes_in_order() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    for sq in 10..=19 {
        serve_fragment(&harness.server, sq).await;
    }
    let limits = FetchLimits {
        concurrency: 4,
        requests_per_second: 10_000.0,
    };

    let report = harness.fetch_with(limits, 10, 19).await.unwrap();

    assert_eq!(report, complete(10));
    let output = harness.data.path().join("joined");
    harness
        .store()
        .concat(harness.stream_id, ITAG, 10, 19, &output)
        .unwrap();
    let expected: Vec<u8> = (10..=19).flat_map(fragment_bytes).collect();
    assert_eq!(fs::read(output).unwrap(), expected);
    assert_eq!(harness.resolver.calls(), 1);
}

#[tokio::test]
async fn forbidden_refreshes_the_url_once_and_succeeds() {
    let harness = Harness::new(IpPolicy::V6First).await;
    Mock::given(query_param("call", "1"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&harness.server)
        .await;
    Mock::given(query_param("call", "2"))
        .respond_with(body(5))
        .mount(&harness.server)
        .await;

    let report = harness.fetch(5, 5).await.unwrap();

    assert_eq!(report, complete(1));
    assert_eq!(harness.resolver.calls(), 2);
    assert_eq!(harness.active_family().await, IpFamily::V6);
}

#[tokio::test]
async fn repeated_refusal_switches_family_and_drops_the_old_lease() {
    let harness = Harness::new(IpPolicy::V6First).await;
    Mock::given(query_param("family", "ipv6"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&harness.server)
        .await;
    Mock::given(query_param("family", "ipv4"))
        .respond_with(body(5))
        .mount(&harness.server)
        .await;

    let report = harness.fetch(5, 5).await.unwrap();

    assert_eq!(report, complete(1));
    assert_eq!(harness.resolver.calls(), 3);
    assert_eq!(harness.active_family().await, IpFamily::V4);
}

#[tokio::test]
async fn refusal_after_the_switch_stops_with_an_error() {
    let harness = Harness::new(IpPolicy::V6First).await;
    Mock::given(query_param("sq", "5"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&harness.server)
        .await;

    let error = harness.fetch(5, 5).await.unwrap_err();

    assert!(matches!(error, AcquireError::FamilyExhausted(IpFamily::V4)));
}

#[tokio::test]
async fn single_family_policy_stops_after_the_refresh() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    Mock::given(query_param("sq", "5"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&harness.server)
        .await;

    let error = harness.fetch(5, 5).await.unwrap_err();

    assert!(matches!(error, AcquireError::FamilyExhausted(IpFamily::V4)));
    assert_eq!(harness.resolver.calls(), 2);
}

#[tokio::test]
async fn concurrent_refusals_of_one_lease_count_once() {
    let harness = Harness::new(IpPolicy::V6First).await;
    Mock::given(query_param("call", "1"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&harness.server)
        .await;
    serve_all(&harness.server).await;
    let limits = FetchLimits {
        concurrency: 4,
        requests_per_second: 10_000.0,
    };

    let report = harness.fetch_with(limits, 1, 4).await.unwrap();

    assert_eq!(report, complete(4));
    assert_eq!(harness.active_family().await, IpFamily::V6);
    assert_eq!(harness.resolver.calls(), 2);
}

#[tokio::test]
async fn service_unavailable_is_retried() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    Mock::given(query_param("sq", "5"))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(2)
        .mount(&harness.server)
        .await;
    serve_fragment(&harness.server, 5).await;

    let report = harness.fetch(5, 5).await.unwrap();

    assert_eq!(report, complete(1));
    assert_eq!(harness.stored(5), fragment_bytes(5));
}

#[tokio::test]
async fn persistent_failure_is_reported_and_the_rest_continues() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    Mock::given(query_param("sq", "5"))
        .respond_with(ResponseTemplate::new(503))
        .expect(4)
        .mount(&harness.server)
        .await;
    serve_fragment(&harness.server, 6).await;

    let report = harness.fetch(5, 6).await.unwrap();

    assert_eq!(
        report,
        FetchReport {
            fetched: 1,
            failed: vec![5],
            ..FetchReport::default()
        }
    );
    assert!(!report.is_complete());
}

#[tokio::test]
async fn empty_200_is_retried_and_never_stored() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    Mock::given(query_param("sq", "5"))
        .respond_with(ResponseTemplate::new(200))
        .up_to_n_times(1)
        .mount(&harness.server)
        .await;
    serve_fragment(&harness.server, 5).await;

    let report = harness.fetch(5, 5).await.unwrap();

    assert_eq!(report, complete(1));
    assert_eq!(harness.stored(5), fragment_bytes(5));
}

#[tokio::test]
async fn repeated_empty_200_refreshes_the_url() {
    let harness = Harness::new(IpPolicy::V6First).await;
    Mock::given(query_param("call", "1"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&harness.server)
        .await;
    Mock::given(query_param("call", "2"))
        .respond_with(body(5))
        .mount(&harness.server)
        .await;

    let report = harness.fetch(5, 5).await.unwrap();

    assert_eq!(report, complete(1));
    assert_eq!(harness.resolver.calls(), 2);
}

#[tokio::test]
async fn lease_is_reused_while_fresh() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    serve_all(&harness.server).await;

    harness.fetch(1, 3).await.unwrap();

    assert_eq!(harness.resolver.calls(), 1);
}

#[tokio::test]
async fn lease_is_replaced_before_it_expires() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    serve_all(&harness.server).await;
    let fetcher = harness.fetcher(FetchLimits::default());
    let session = &fetcher.session;

    session.lease().await.unwrap();
    harness
        .now
        .store(START + LONG_LIFETIME - 1800 - 1, Ordering::SeqCst);
    session.lease().await.unwrap();
    assert_eq!(harness.resolver.calls(), 1);

    harness.now.fetch_add(1, Ordering::SeqCst);
    session.lease().await.unwrap();
    assert_eq!(harness.resolver.calls(), 2);
}

#[tokio::test]
async fn expired_url_refused_by_the_origin_is_refreshed() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    harness.resolver.expire.store(START + 60, Ordering::SeqCst);
    Mock::given(query_param("call", "1"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&harness.server)
        .await;
    serve_fragment(&harness.server, 5).await;

    let report = harness.fetch(5, 5).await.unwrap();

    assert_eq!(report, complete(1));
    assert_eq!(harness.resolver.calls(), 2);
}

#[tokio::test]
async fn short_lived_url_is_not_resolved_for_every_request() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    harness.resolver.expire.store(START + 60, Ordering::SeqCst);
    serve_all(&harness.server).await;

    harness.fetch(1, 5).await.unwrap();

    assert_eq!(harness.resolver.calls(), 1);
}

#[tokio::test]
async fn stored_fragments_are_not_requested_again() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    harness.add_row(6);
    Mock::given(query_param("sq", "6"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&harness.server)
        .await;
    serve_fragment(&harness.server, 5).await;
    serve_fragment(&harness.server, 7).await;

    let report = harness.fetch(5, 7).await.unwrap();

    assert_eq!(
        report,
        FetchReport {
            already_stored: 1,
            fetched: 2,
            ..FetchReport::default()
        }
    );
}

#[tokio::test]
async fn interrupted_fetch_resumes_where_it_stopped() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    serve_fragment(&harness.server, 1).await;
    Mock::given(query_param("sq", "2"))
        .respond_with(ResponseTemplate::new(403))
        .up_to_n_times(2)
        .mount(&harness.server)
        .await;
    serve_fragment(&harness.server, 2).await;

    assert!(harness.fetch(1, 2).await.is_err());
    let report = harness.fetch(1, 2).await.unwrap();

    assert_eq!(
        report,
        FetchReport {
            already_stored: 1,
            fetched: 1,
            ..FetchReport::default()
        }
    );
}

#[tokio::test]
async fn fragment_behind_the_head_that_is_gone_is_lost() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    Mock::given(query_param("sq", "5"))
        .respond_with(ResponseTemplate::new(404).insert_header("X-Head-Seqnum", "100"))
        .expect(1)
        .mount(&harness.server)
        .await;

    let report = harness.fetch(5, 5).await.unwrap();

    assert_eq!(report.lost, [5]);
    assert!(!report.is_complete());
}

#[tokio::test]
async fn fragment_past_the_head_is_awaited() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    Mock::given(query_param("sq", "5"))
        .respond_with(ResponseTemplate::new(404).insert_header("X-Head-Seqnum", "4"))
        .up_to_n_times(1)
        .mount(&harness.server)
        .await;
    serve_fragment(&harness.server, 5).await;

    assert_eq!(harness.fetch(5, 5).await.unwrap(), complete(1));
}

#[tokio::test]
async fn bot_check_during_resolution_switches_family() {
    let harness = Harness::new(IpPolicy::V6First).await;
    harness
        .resolver
        .errors
        .lock()
        .unwrap()
        .push_back(ResolveError::BotCheck);
    Mock::given(query_param("family", "ipv4"))
        .respond_with(body(5))
        .mount(&harness.server)
        .await;

    assert_eq!(harness.fetch(5, 5).await.unwrap(), complete(1));
    assert_eq!(harness.active_family().await, IpFamily::V4);
}

#[tokio::test]
async fn unavailable_stream_leaves_the_family_alone() {
    let harness = Harness::new(IpPolicy::V6First).await;
    harness
        .resolver
        .errors
        .lock()
        .unwrap()
        .push_back(ResolveError::Unavailable("private".to_string()));

    let error = harness.fetch(5, 5).await.unwrap_err();

    assert!(matches!(error, AcquireError::Unavailable(_)));
    assert_eq!(harness.active_family().await, IpFamily::V6);
}

#[tokio::test]
async fn resolver_failures_are_retried_then_reported() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    harness
        .resolver
        .errors
        .lock()
        .unwrap()
        .extend((0..4).map(|_| ResolveError::Failed("broken".to_string())));

    let error = harness.fetch(5, 5).await.unwrap_err();

    assert!(matches!(error, AcquireError::Resolve(_)));
    assert_eq!(harness.resolver.calls(), 4);
}

#[tokio::test]
async fn preferred_family_returns_after_the_avoidance_period() {
    let harness = Harness::new(IpPolicy::V6First).await;
    harness
        .resolver
        .errors
        .lock()
        .unwrap()
        .push_back(ResolveError::BotCheck);
    serve_all(&harness.server).await;
    harness.fetch(1, 1).await.unwrap();
    assert_eq!(harness.active_family().await, IpFamily::V4);

    harness.now.fetch_add(6 * 3600, Ordering::SeqCst);
    harness.fetch(2, 2).await.unwrap();

    assert_eq!(harness.active_family().await, IpFamily::V6);
}

#[tokio::test]
async fn requests_are_paced() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    serve_all(&harness.server).await;
    let limits = FetchLimits {
        concurrency: 4,
        requests_per_second: 100.0,
    };

    let started = Instant::now();
    harness.fetch_with(limits, 1, 5).await.unwrap();

    assert!(started.elapsed() >= Duration::from_millis(40));
}

#[tokio::test]
async fn invalid_requests_are_rejected_before_any_network_use() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    let bad_limits = [
        FetchLimits {
            concurrency: 0,
            requests_per_second: 1.0,
        },
        FetchLimits {
            concurrency: 1,
            requests_per_second: 0.0,
        },
        FetchLimits {
            concurrency: 1,
            requests_per_second: f64::NAN,
        },
        FetchLimits {
            concurrency: 1,
            requests_per_second: 1e-300,
        },
    ];

    assert!(matches!(
        harness.fetch(2, 1).await,
        Err(AcquireError::InvalidRequest(_))
    ));
    for limits in bad_limits {
        assert!(matches!(
            harness.fetch_with(limits, 1, 2).await,
            Err(AcquireError::InvalidRequest(_))
        ));
    }
    assert_eq!(harness.resolver.calls(), 0);
}

#[tokio::test]
async fn one_refused_fragment_cannot_keep_resolving_while_others_succeed() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    Mock::given(query_param("sq", "1"))
        .respond_with(ResponseTemplate::new(403).set_delay(Duration::from_millis(100)))
        .mount(&harness.server)
        .await;
    serve_all(&harness.server).await;
    let limits = FetchLimits {
        concurrency: 2,
        requests_per_second: 10_000.0,
    };

    let _ = harness.fetch_with(limits, 1, 60).await;

    assert!(
        harness.resolver.calls() <= 5,
        "{}",
        harness.resolver.calls()
    );
}

#[tokio::test]
async fn timing_comes_from_the_fragment_bytes() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    serve_fragment(&harness.server, 7).await;

    harness.fetch(7, 7).await.unwrap();

    assert_eq!(
        harness.rows(7, 7),
        [FragmentRow {
            stream_id: harness.stream_id,
            itag: ITAG,
            sq: 7,
            stream_time: 7.0,
            duration: 5.0,
            size: fragment_bytes(7).len() as u64,
            fetched_at: START as i64 * 1_000,
        }]
    );
}

#[tokio::test]
async fn a_200_that_is_not_a_fragment_is_retried_and_never_stored() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    Mock::given(query_param("sq", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"<html>".to_vec()))
        .up_to_n_times(1)
        .mount(&harness.server)
        .await;
    serve_fragment(&harness.server, 5).await;

    assert_eq!(harness.fetch(5, 5).await.unwrap(), complete(1));
    assert_eq!(harness.stored(5), fragment_bytes(5));
}

#[tokio::test]
async fn head_samples_are_taken_at_most_once_a_minute() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    for (sq, walltime) in [(1, 1_000_000), (2, 1_030_000), (3, 1_060_000)] {
        Mock::given(query_param("sq", sq.to_string()))
            .respond_with(
                body(sq)
                    .insert_header("X-Head-Seqnum", "100")
                    .insert_header("X-Head-Time-Millis", "500250")
                    .insert_header("X-Walltime-Ms", walltime.to_string()),
            )
            .mount(&harness.server)
            .await;
    }

    harness.fetch(1, 3).await.unwrap();

    let database = harness.database.lock().unwrap();
    assert_eq!(
        database.latest_head_sample(harness.stream_id).unwrap(),
        Some(HeadSample {
            stream_id: harness.stream_id,
            observed_at: 1_060_000,
            head_sq: 100,
            head_time: 500.25,
        })
    );
    let samples: i64 = database
        .connection()
        .query_row("SELECT COUNT(*) FROM head_samples", [], |row| row.get(0))
        .unwrap();
    assert_eq!(samples, 2);
}

// Following fetches one fragment at a time, so the limit must outlast one range.
#[tokio::test]
async fn head_samples_stay_once_a_minute_across_ranges() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    for (sq, walltime) in [(1, 1_000_000), (2, 1_030_000), (3, 1_060_000)] {
        Mock::given(query_param("sq", sq.to_string()))
            .respond_with(
                body(sq)
                    .insert_header("X-Head-Seqnum", "100")
                    .insert_header("X-Head-Time-Millis", "500250")
                    .insert_header("X-Walltime-Ms", walltime.to_string()),
            )
            .mount(&harness.server)
            .await;
    }

    for sq in 1..=3 {
        harness.fetch(sq, sq).await.unwrap();
    }

    let samples: i64 = harness
        .database
        .lock()
        .unwrap()
        .connection()
        .query_row("SELECT COUNT(*) FROM head_samples", [], |row| row.get(0))
        .unwrap();
    assert_eq!(samples, 2);
}

#[tokio::test]
async fn ranges_longer_than_the_dvr_are_rejected() {
    let harness = Harness::new(IpPolicy::V4Only).await;

    for (first, last) in [(0, u64::MAX), (1, 1_000_000)] {
        assert!(matches!(
            harness.fetch(first, last).await,
            Err(AcquireError::InvalidRequest(_))
        ));
    }
}

#[tokio::test]
async fn errors_never_mention_the_url() {
    let harness = Harness::new(IpPolicy::V4Only).await;
    Mock::given(query_param("sq", "5"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&harness.server)
        .await;

    let error = harness.fetch(5, 5).await.unwrap_err();

    let shown = format!("{error} {error:?}");
    assert!(!shown.contains("videoplayback"), "{shown}");
    assert!(!format!("{:?}", harness.fetcher(FetchLimits::default())).contains("videoplayback"));
}

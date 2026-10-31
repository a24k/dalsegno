//! Range fetches against a mock origin: the failures listed in the M1 exit criteria.

use dalsegno_app::error::AcquireError;
use dalsegno_app::family::{FamilyController, SharedFamily};
use dalsegno_app::fetch::{
    FetchLimits, FetchRange, FetchReport, Fetcher, RetryPolicy, fetch_range,
};
use dalsegno_app::session::{Clock, Session};
use dalsegno_core::redact::Redacted;
use dalsegno_core::stream::{IpFamily, IpPolicy, VideoId};
use dalsegno_io::fragments::FragmentStore;
use dalsegno_io::http::{BindAddresses, FragmentClient};
use dalsegno_io::ytdlp::{ResolveError, Resolver, StreamInfo};
use std::collections::VecDeque;
use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use wiremock::matchers::query_param;
use wiremock::{Mock, MockServer, ResponseTemplate};

const ITAG: u32 = 140;
const START: u64 = 1_000_000;
const LONG_LIFETIME: u64 = 6 * 3600;

#[derive(Debug)]
struct FakeResolver {
    base: String,
    expire: AtomicU64,
    calls: AtomicU64,
    errors: Mutex<VecDeque<ResolveError>>,
}

impl FakeResolver {
    fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Resolver for FakeResolver {
    async fn resolve(
        &self,
        _video: &VideoId,
        _itag: u32,
        family: IpFamily,
    ) -> Result<Redacted<String>, ResolveError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(error) = self.errors.lock().unwrap().pop_front() {
            return Err(error);
        }
        let expire = self.expire.load(Ordering::SeqCst);
        Ok(Redacted::new(format!(
            "{}/videoplayback?call={call}&family={family}&expire={expire}",
            self.base
        )))
    }

    async fn stream_info(
        &self,
        _video: &VideoId,
        _family: IpFamily,
    ) -> Result<StreamInfo, ResolveError> {
        Err(ResolveError::Failed("not used".to_string()))
    }
}

struct Harness {
    server: MockServer,
    resolver: Arc<FakeResolver>,
    family: SharedFamily,
    now: Arc<AtomicU64>,
    data: TempDir,
}

impl Harness {
    async fn new(policy: IpPolicy) -> Self {
        let server = MockServer::start().await;
        let resolver = Arc::new(FakeResolver {
            base: server.uri(),
            expire: AtomicU64::new(START + LONG_LIFETIME),
            calls: AtomicU64::new(0),
            errors: Mutex::new(VecDeque::new()),
        });
        Self {
            server,
            resolver,
            family: FamilyController::new(policy, true).shared(),
            now: Arc::new(AtomicU64::new(START)),
            data: tempfile::tempdir().unwrap(),
        }
    }

    fn video() -> VideoId {
        VideoId::parse("abcDEF123_-").unwrap()
    }

    fn store(&self) -> FragmentStore {
        FragmentStore::new(self.data.path())
    }

    // Both families bind to loopback, because the mock listens on IPv4 only.
    fn fetcher(&self, limits: FetchLimits) -> Fetcher<FakeResolver> {
        let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let now = Arc::clone(&self.now);
        let clock: Clock = Arc::new(move || now.load(Ordering::SeqCst));
        Fetcher {
            session: Session::new(
                Arc::clone(&self.resolver),
                Self::video(),
                ITAG,
                Arc::clone(&self.family),
                clock,
                Duration::from_millis(1),
            ),
            client: FragmentClient::new(BindAddresses {
                v4: loopback,
                v6: loopback,
            })
            .unwrap(),
            store: self.store(),
            limits,
            retry: RetryPolicy {
                base_delay: Duration::from_millis(1),
                max_attempts: 4,
                not_yet_delay: Duration::from_millis(1),
            },
        }
    }

    async fn fetch(&self, first: u64, last: u64) -> Result<FetchReport, AcquireError> {
        let limits = FetchLimits {
            concurrency: 1,
            requests_per_second: 10_000.0,
        };
        self.fetch_with(limits, first, last).await
    }

    async fn fetch_with(
        &self,
        limits: FetchLimits,
        first: u64,
        last: u64,
    ) -> Result<FetchReport, AcquireError> {
        let range = FetchRange {
            video: Self::video(),
            itag: ITAG,
            first,
            last,
        };
        fetch_range(self.fetcher(limits), range).await
    }

    fn stored(&self, sq: u64) -> Vec<u8> {
        self.store().read(&Self::video(), ITAG, sq).unwrap()
    }

    async fn active_family(&self) -> IpFamily {
        self.family.lock().await.active()
    }
}

fn body(sq: u64) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_bytes(format!("fragment-{sq};").into_bytes())
}

async fn serve_fragment(server: &MockServer, sq: u64) {
    Mock::given(query_param("sq", sq.to_string()))
        .respond_with(body(sq))
        .mount(server)
        .await;
}

async fn serve_all(server: &MockServer) {
    Mock::given(query_param("family", "ipv4"))
        .respond_with(body(0))
        .mount(server)
        .await;
    Mock::given(query_param("family", "ipv6"))
        .respond_with(body(0))
        .mount(server)
        .await;
}

fn complete(fetched: u64) -> FetchReport {
    FetchReport {
        fetched,
        ..FetchReport::default()
    }
}

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
        .concat(&Harness::video(), ITAG, 10, 19, &output)
        .unwrap();
    let expected: String = (10..=19).map(|sq| format!("fragment-{sq};")).collect();
    assert_eq!(fs::read(output).unwrap(), expected.into_bytes());
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
    assert_eq!(harness.stored(5), b"fragment-5;");
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
    assert_eq!(harness.stored(5), b"fragment-5;");
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
    harness
        .store()
        .write(&Harness::video(), ITAG, 6, b"kept")
        .unwrap();
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
    assert_eq!(harness.stored(6), b"kept");
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

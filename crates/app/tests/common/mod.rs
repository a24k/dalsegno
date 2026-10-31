//! A mock origin and a fake resolver, shared by the fetch tests.
#![allow(dead_code, unused_imports)]

pub use dalsegno_app::error::AcquireError;
pub use dalsegno_app::family::{FamilyController, SharedFamily};
pub use dalsegno_app::fetch::{
    FetchLimits, FetchRange, FetchReport, Fetcher, Ledger, RetryPolicy, SharedDatabase, fetch_range,
};
pub use dalsegno_app::session::{Clock, Session};
pub use dalsegno_core::redact::Redacted;
pub use dalsegno_core::stream::{IpFamily, IpPolicy, VideoId};
pub use dalsegno_io::fragments::FragmentStore;
pub use dalsegno_io::http::{BindAddresses, FragmentClient};
pub use dalsegno_io::ytdlp::{ResolveError, Resolver, StreamInfo};
pub use dalsegno_store::database::Database;
pub use dalsegno_store::fragments::{FragmentRow, HeadSample};
pub use dalsegno_store::streams::NewStream;
pub use std::collections::VecDeque;
pub use std::fs;
pub use std::net::{IpAddr, Ipv4Addr};
pub use std::sync::atomic::{AtomicU64, Ordering};
pub use std::sync::{Arc, Mutex};
pub use std::time::{Duration, Instant};
pub use tempfile::TempDir;
pub use wiremock::matchers::query_param;
pub use wiremock::{Mock, MockServer, ResponseTemplate};

pub const ITAG: u32 = 140;
pub const START: u64 = 1_000_000;
pub const LONG_LIFETIME: u64 = 6 * 3600;

#[derive(Debug)]
pub struct FakeResolver {
    pub base: String,
    pub expire: AtomicU64,
    pub calls: AtomicU64,
    pub errors: Mutex<VecDeque<ResolveError>>,
}

impl FakeResolver {
    pub fn calls(&self) -> u64 {
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

pub struct Harness {
    pub server: MockServer,
    pub resolver: Arc<FakeResolver>,
    pub family: SharedFamily,
    pub now: Arc<AtomicU64>,
    pub data: TempDir,
    pub database: SharedDatabase,
    pub stream_id: i64,
}

impl Harness {
    pub async fn new(policy: IpPolicy) -> Self {
        let server = MockServer::start().await;
        let resolver = Arc::new(FakeResolver {
            base: server.uri(),
            expire: AtomicU64::new(START + LONG_LIFETIME),
            calls: AtomicU64::new(0),
            errors: Mutex::new(VecDeque::new()),
        });
        let database = Database::open_in_memory().unwrap();
        let stream_id = database
            .insert_stream(&NewStream {
                video_id: Self::video().to_string(),
                title: "Example TV".to_string(),
                channel_id: None,
                channel_name: None,
                thumbnail_url: None,
                video_itag: None,
                audio_itag: ITAG,
                fragment_len: 5.0,
                registered_at: 0,
            })
            .unwrap();
        Self {
            database: Arc::new(Mutex::new(database)),
            stream_id,
            server,
            resolver,
            family: FamilyController::new(policy, true).shared(),
            now: Arc::new(AtomicU64::new(START)),
            data: tempfile::tempdir().unwrap(),
        }
    }

    pub fn video() -> VideoId {
        VideoId::parse("abcDEF123_-").unwrap()
    }

    pub fn store(&self) -> FragmentStore {
        FragmentStore::new(self.data.path())
    }

    // Both families bind to loopback, because the mock listens on IPv4 only.
    pub fn fetcher(&self, limits: FetchLimits) -> Fetcher<FakeResolver> {
        let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let now = Arc::clone(&self.now);
        let clock: Clock = Arc::new(move || now.load(Ordering::SeqCst));
        Fetcher {
            ledger: Ledger {
                database: Arc::clone(&self.database),
                stream_id: self.stream_id,
                itag: ITAG,
                fragment_len: 5.0,
            },
            clock: Arc::clone(&clock),
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

    pub async fn fetch(&self, first: u64, last: u64) -> Result<FetchReport, AcquireError> {
        let limits = FetchLimits {
            concurrency: 1,
            requests_per_second: 10_000.0,
        };
        self.fetch_with(limits, first, last).await
    }

    pub async fn fetch_with(
        &self,
        limits: FetchLimits,
        first: u64,
        last: u64,
    ) -> Result<FetchReport, AcquireError> {
        let range = FetchRange {
            stream_id: self.stream_id,
            itag: ITAG,
            first,
            last,
        };
        fetch_range(Arc::new(self.fetcher(limits)), range).await
    }

    pub fn stored(&self, sq: u64) -> Vec<u8> {
        self.store().read(self.stream_id, ITAG, sq).unwrap()
    }

    pub fn add_row(&self, sq: u64) {
        let row = FragmentRow {
            stream_id: self.stream_id,
            itag: ITAG,
            sq,
            stream_time: 0.0,
            duration: 5.0,
            size: 1,
            fetched_at: 0,
        };
        self.database.lock().unwrap().put_fragment(&row).unwrap();
    }

    pub fn rows(&self, first: u64, last: u64) -> Vec<FragmentRow> {
        self.database
            .lock()
            .unwrap()
            .fragments_in_range(self.stream_id, ITAG, first, last)
            .unwrap()
    }

    pub async fn active_family(&self) -> IpFamily {
        self.family.lock().await.active()
    }
}

/// A minimal WebM fragment whose Cluster Timecode is `sq` seconds, followed by padding.
pub fn fragment_bytes(sq: u64) -> Vec<u8> {
    const EBML_HEADER: [u8; 5] = [0x1A, 0x45, 0xDF, 0xA3, 0x80];
    const CLUSTER: [u8; 12] = [
        0x1F, 0x43, 0xB6, 0x75, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF,
    ];
    let timecode = (sq * 1_000).to_be_bytes();
    [&EBML_HEADER[..], &CLUSTER, &[0xE7, 0x88], &timecode].concat()
}

pub fn body(sq: u64) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_bytes(fragment_bytes(sq))
}

pub async fn serve_fragment(server: &MockServer, sq: u64) {
    Mock::given(query_param("sq", sq.to_string()))
        .respond_with(body(sq))
        .mount(server)
        .await;
}

pub async fn serve_all(server: &MockServer) {
    Mock::given(query_param("family", "ipv4"))
        .respond_with(body(0))
        .mount(server)
        .await;
    Mock::given(query_param("family", "ipv6"))
        .respond_with(body(0))
        .mount(server)
        .await;
}

pub fn complete(fetched: u64) -> FetchReport {
    FetchReport {
        fetched,
        ..FetchReport::default()
    }
}

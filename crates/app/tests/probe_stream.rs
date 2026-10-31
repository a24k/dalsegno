//! Stream probing against a mock origin.

use dalsegno_app::error::AcquireError;
use dalsegno_app::family::{FamilyController, SharedFamily};
use dalsegno_app::probe::{ProbeReport, ProbeTools, probe_stream, read_head};
use dalsegno_app::session::Clock;
use dalsegno_core::format::FormatInfo;
use dalsegno_core::redact::Redacted;
use dalsegno_core::stream::{IpFamily, IpPolicy, VideoId};
use dalsegno_io::http::{BindAddresses, FragmentClient};
use dalsegno_io::tools::find_program;
use dalsegno_io::ytdlp::{ResolveError, Resolver, StreamFormat, StreamInfo};
use dalsegno_test_support::require_fixtures;
use std::collections::VecDeque;
use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use wiremock::matchers::{path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HEAD: u64 = 100;
const AAC_LC: &str = "mp4a.40.2";

struct FakeResolver {
    info: StreamInfo,
    errors: Mutex<VecDeque<ResolveError>>,
    families: Mutex<Vec<IpFamily>>,
}

impl FakeResolver {
    fn new(info: StreamInfo) -> Self {
        Self {
            info,
            errors: Mutex::new(VecDeque::new()),
            families: Mutex::new(Vec::new()),
        }
    }

    fn failing_with(info: StreamInfo, error: ResolveError) -> Self {
        let resolver = Self::new(info);
        resolver.errors.lock().unwrap().push_back(error);
        resolver
    }
}

impl Resolver for FakeResolver {
    async fn resolve(
        &self,
        _video: &VideoId,
        _itag: u32,
        _family: IpFamily,
    ) -> Result<Redacted<String>, ResolveError> {
        Err(ResolveError::Failed("not used".to_string()))
    }

    async fn stream_info(
        &self,
        _video: &VideoId,
        family: IpFamily,
    ) -> Result<StreamInfo, ResolveError> {
        self.families.lock().unwrap().push(family);
        match self.errors.lock().unwrap().pop_front() {
            Some(error) => Err(error),
            None => Ok(self.info.clone()),
        }
    }
}

fn stream_format(
    server: &MockServer,
    itag: u32,
    acodec: Option<&str>,
    height: u32,
) -> StreamFormat {
    StreamFormat {
        info: FormatInfo {
            itag,
            ext: "bin".to_string(),
            vcodec: acodec.is_none().then(|| "vp9".to_string()),
            acodec: acodec.map(str::to_string),
            height: acodec.is_none().then_some(height),
            fps: None,
            tbr: None,
            has_drm: false,
        },
        url: Redacted::new(format!("{}/{itag}?itag={itag}", server.uri())),
    }
}

fn info(formats: Vec<StreamFormat>) -> StreamInfo {
    StreamInfo {
        title: "Example TV -An Archive-".to_string(),
        channel: Some("Example".to_string()),
        channel_id: Some("UCexample".to_string()),
        thumbnail: None,
        formats,
    }
}

fn audio_and_video(server: &MockServer) -> StreamInfo {
    info(vec![
        stream_format(server, 140, Some(AAC_LC), 0),
        stream_format(server, 248, None, 1080),
    ])
}

fn missing_tools() -> ProbeTools {
    ProbeTools {
        ffprobe: PathBuf::from("/definitely/not/ffprobe"),
        ffmpeg: PathBuf::from("/definitely/not/ffmpeg"),
    }
}

fn client() -> FragmentClient {
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
    FragmentClient::new(BindAddresses {
        v4: loopback,
        v6: loopback,
    })
    .unwrap()
}

async fn origin() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(query_param("sq", "1"))
        .respond_with(ResponseTemplate::new(404).insert_header("X-Head-Seqnum", HEAD.to_string()))
        .mount(&server)
        .await;
    server
}

async fn serve_garbage(server: &MockServer) {
    Mock::given(wiremock::matchers::method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"garbage".to_vec()))
        .mount(server)
        .await;
}

async fn probe(
    resolver: &FakeResolver,
    tools: &ProbeTools,
    family: &SharedFamily,
) -> Result<ProbeReport, AcquireError> {
    let video = VideoId::parse("abcDEF123_-").unwrap();
    let clock: Clock = Arc::new(|| 1_000);
    probe_stream(resolver, &client(), tools, &video, family, &clock).await
}

fn family(policy: IpPolicy) -> SharedFamily {
    FamilyController::new(policy, true).shared()
}

#[tokio::test]
async fn damaged_fragments_reject_formats_but_metadata_is_reported() {
    let server = origin().await;
    serve_garbage(&server).await;
    let resolver = FakeResolver::new(audio_and_video(&server));

    let report = probe(&resolver, &missing_tools(), &family(IpPolicy::V4Only))
        .await
        .unwrap();

    assert_eq!(report.short_name, "Example TV");
    assert_eq!(report.channel_id.as_deref(), Some("UCexample"));
    assert_eq!(report.head_sq, HEAD);
    assert_eq!(report.family, IpFamily::V4);
    assert_eq!((report.audio, report.video), (None, None));
    assert_eq!(report.fragment_seconds, None);
    let rejected: Vec<u32> = report.rejected.iter().map(|item| item.itag).collect();
    assert_eq!(rejected, [140, 248]);
}

#[tokio::test]
async fn refused_fragments_name_the_status() {
    let server = origin().await;
    Mock::given(path("/140"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let resolver = FakeResolver::new(info(vec![stream_format(&server, 140, Some(AAC_LC), 0)]));

    let report = probe(&resolver, &missing_tools(), &family(IpPolicy::V4Only))
        .await
        .unwrap();

    assert_eq!(report.rejected[0].reason, "status 403 with 0 bytes");
}

#[tokio::test]
async fn video_candidates_are_tried_best_first_up_to_a_limit() {
    let server = origin().await;
    serve_garbage(&server).await;
    let formats = (1..=6)
        .map(|index| stream_format(&server, index, None, index * 100))
        .collect();
    let resolver = FakeResolver::new(info(formats));

    let report = probe(&resolver, &missing_tools(), &family(IpPolicy::V4Only))
        .await
        .unwrap();

    let rejected: Vec<u32> = report.rejected.iter().map(|item| item.itag).collect();
    assert_eq!(rejected, [6, 5, 4, 3]);
}

#[tokio::test]
async fn bot_check_moves_the_probe_to_the_other_family() {
    let server = origin().await;
    serve_garbage(&server).await;
    let resolver = FakeResolver::failing_with(audio_and_video(&server), ResolveError::BotCheck);

    let report = probe(&resolver, &missing_tools(), &family(IpPolicy::V6First))
        .await
        .unwrap();

    assert_eq!(report.family, IpFamily::V4);
    assert_eq!(
        *resolver.families.lock().unwrap(),
        [IpFamily::V6, IpFamily::V4]
    );
}

#[tokio::test]
async fn bot_check_with_no_other_family_stops() {
    let server = origin().await;
    let resolver = FakeResolver::failing_with(audio_and_video(&server), ResolveError::BotCheck);

    let error = probe(&resolver, &missing_tools(), &family(IpPolicy::V4Only))
        .await
        .unwrap_err();

    assert!(matches!(error, AcquireError::FamilyExhausted(IpFamily::V4)));
}

#[tokio::test]
async fn unavailable_and_failing_lookups_are_reported() {
    let server = origin().await;
    let gone = FakeResolver::failing_with(
        audio_and_video(&server),
        ResolveError::Unavailable("private".to_string()),
    );
    let broken = FakeResolver::failing_with(
        audio_and_video(&server),
        ResolveError::Failed("broken".to_string()),
    );

    assert!(matches!(
        probe(&gone, &missing_tools(), &family(IpPolicy::V4Only)).await,
        Err(AcquireError::Unavailable(_))
    ));
    assert!(matches!(
        probe(&broken, &missing_tools(), &family(IpPolicy::V4Only)).await,
        Err(AcquireError::Resolve(_))
    ));
}

#[tokio::test]
async fn a_stream_without_live_formats_is_an_error() {
    let resolver = FakeResolver::new(info(Vec::new()));

    let error = probe(&resolver, &missing_tools(), &family(IpPolicy::V4Only))
        .await
        .unwrap_err();

    assert!(matches!(error, AcquireError::InvalidRequest(_)));
}

#[tokio::test]
async fn an_origin_without_a_head_header_is_an_error() {
    let server = MockServer::start().await;
    serve_garbage(&server).await;
    let url = Redacted::new(format!("{}/140?itag=140", server.uri()));

    let error = read_head(&client(), &url, IpFamily::V4).await.unwrap_err();

    assert!(matches!(error, AcquireError::NoHead));
}

#[tokio::test]
async fn a_stream_too_short_to_probe_rejects_its_formats() {
    let server = MockServer::start().await;
    Mock::given(query_param("sq", "1"))
        .respond_with(ResponseTemplate::new(200).insert_header("X-Head-Seqnum", "1"))
        .mount(&server)
        .await;
    let resolver = FakeResolver::new(audio_and_video(&server));

    let report = probe(&resolver, &missing_tools(), &family(IpPolicy::V4Only))
        .await
        .unwrap();

    assert_eq!(
        report.rejected[0].reason,
        "the stream is too short to probe"
    );
}

fn first_two_numbered_files(directory: &Path) -> [Vec<u8>; 2] {
    let mut numbers: Vec<u64> = fs::read_dir(directory)
        .unwrap()
        .filter_map(|entry| entry.unwrap().file_name().to_str()?.parse().ok())
        .collect();
    numbers.sort_unstable();
    [numbers[0], numbers[1]].map(|sq| fs::read(directory.join(sq.to_string())).unwrap())
}

async fn serve_pair(server: &MockServer, route: &str, pair: [Vec<u8>; 2]) {
    for (sq, bytes) in [HEAD - 2, HEAD - 1].into_iter().zip(pair) {
        Mock::given(path(route.to_string()))
            .and(query_param("sq", sq.to_string()))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes))
            .mount(server)
            .await;
    }
}

#[tokio::test]
async fn real_fragments_choose_both_formats() {
    let fixtures = require_fixtures!();
    let (Some(ffprobe), Some(ffmpeg)) = (find_program("ffprobe"), find_program("ffmpeg")) else {
        return;
    };
    let server = origin().await;
    serve_pair(
        &server,
        "/140",
        first_two_numbered_files(&fixtures.join("frag-140")),
    )
    .await;
    serve_pair(
        &server,
        "/248",
        first_two_numbered_files(&fixtures.join("frag-248")),
    )
    .await;
    let resolver = FakeResolver::new(audio_and_video(&server));
    let tools = ProbeTools { ffprobe, ffmpeg };

    let report = probe(&resolver, &tools, &family(IpPolicy::V4Only))
        .await
        .unwrap();

    assert_eq!(report.rejected, []);
    assert_eq!(report.audio.unwrap().itag, 140);
    assert_eq!(report.video.unwrap().itag, 248);
    assert!(report.fragment_seconds.is_some());
}

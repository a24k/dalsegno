//! The use cases the CLI calls in process, and the API will wrap at milestone 5 (D-17).
//!
//! Text input is validated here, before any external call.

use crate::doctor::{DoctorReport, run_doctor};
use crate::error::AcquireError;
use crate::family::{FamilyController, SharedFamily};
use crate::fetch::{FetchLimits, FetchRange, FetchReport, Fetcher, RetryPolicy, fetch_range};
use crate::probe::{ProbeReport, ProbeTools, probe_stream};
use crate::session::{Clock, Session, system_clock};
pub use dalsegno_core::stream::IpPolicy;

use dalsegno_core::stream::VideoId;
use dalsegno_io::fragments::FragmentStore;
use dalsegno_io::http::{BindAddresses, FragmentClient};
use dalsegno_io::tools::{find_program, has_ipv6_route};
use dalsegno_io::ytdlp::YtDlp;
use std::path::PathBuf;
use std::sync::Arc;

const YT_DLP: &str = "yt-dlp";

/// Settings shared by the acquisition use cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquisitionOptions {
    /// IP family policy.
    pub policy: IpPolicy,
    /// Cookie file for yt-dlp.
    pub cookies: Option<PathBuf>,
}

/// One range to fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRequest {
    /// Video ID or URL.
    pub stream: String,
    /// Format.
    pub itag: u32,
    /// First `sq`.
    pub first: u64,
    /// Last `sq`.
    pub last: u64,
    /// Data directory.
    pub data_dir: PathBuf,
    /// Where to join the range, once it is complete.
    pub output: Option<PathBuf>,
}

/// The acquisition use cases, wired to the real adapters.
#[derive(Debug, Clone)]
pub struct Acquisition {
    resolver: Arc<YtDlp>,
    client: FragmentClient,
    family: SharedFamily,
    has_ipv6: bool,
}

impl Acquisition {
    /// Detects the IPv6 route and builds the adapters.
    pub fn new(options: AcquisitionOptions) -> Result<Self, AcquireError> {
        let has_ipv6 = has_ipv6_route();
        let program = find_program(YT_DLP).unwrap_or_else(|| PathBuf::from(YT_DLP));
        Ok(Self {
            resolver: Arc::new(YtDlp::new(program, options.cookies)),
            client: FragmentClient::new(BindAddresses::default())?,
            family: FamilyController::new(options.policy, has_ipv6).shared(),
            has_ipv6,
        })
    }

    /// Reports tools, routes and the family state, with URL resolution if a stream is given.
    pub async fn doctor(&self, stream: Option<&str>) -> Result<DoctorReport, AcquireError> {
        let video = stream.map(parse_stream).transpose()?;
        let family = self.family.lock().await.clone();
        Ok(run_doctor(
            self.resolver.as_ref(),
            &family,
            self.has_ipv6,
            video.as_ref(),
        )
        .await)
    }

    /// Reads a stream's metadata and checks its formats.
    pub async fn probe(&self, stream: &str) -> Result<ProbeReport, AcquireError> {
        let video = parse_stream(stream)?;
        let tools = ProbeTools {
            ffprobe: require_program("ffprobe")?,
            ffmpeg: require_program("ffmpeg")?,
        };
        let clock = system_clock();
        probe_stream(
            self.resolver.as_ref(),
            &self.client,
            &tools,
            &video,
            &self.family,
            &clock,
        )
        .await
    }

    /// Fetches a range, and joins it when it is complete and an output is set.
    pub async fn fetch(&self, request: FetchRequest) -> Result<FetchReport, AcquireError> {
        let video = parse_stream(&request.stream)?;
        let store = FragmentStore::new(&request.data_dir);
        let clock: Clock = system_clock();
        let session = Session::new(
            Arc::clone(&self.resolver),
            video.clone(),
            request.itag,
            Arc::clone(&self.family),
            clock,
            RetryPolicy::default().base_delay,
        );
        let fetcher = Fetcher {
            session,
            client: self.client.clone(),
            store: store.clone(),
            limits: FetchLimits::default(),
            retry: RetryPolicy::default(),
        };
        let range = FetchRange {
            video: video.clone(),
            itag: request.itag,
            first: request.first,
            last: request.last,
        };
        let report = fetch_range(fetcher, range).await?;
        if let Some(output) = request.output.filter(|_| report.is_complete()) {
            let (itag, first, last) = (request.itag, request.first, request.last);
            tokio::task::spawn_blocking(move || store.concat(&video, itag, first, last, &output))
                .await
                .map_err(|_| AcquireError::Task)??;
        }
        Ok(report)
    }
}

fn parse_stream(input: &str) -> Result<VideoId, AcquireError> {
    VideoId::parse(input).map_err(|_| AcquireError::InvalidRequest("not a video ID or video URL"))
}

fn require_program(name: &'static str) -> Result<PathBuf, AcquireError> {
    find_program(name).ok_or(AcquireError::InvalidRequest(
        "ffprobe and ffmpeg must be installed",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acquisition() -> Acquisition {
        Acquisition::new(AcquisitionOptions {
            policy: IpPolicy::V4Only,
            cookies: None,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn bad_stream_ids_are_rejected_before_any_external_call() {
        let tool = acquisition();
        let request = FetchRequest {
            stream: "--exec=rm".to_string(),
            itag: 140,
            first: 1,
            last: 1,
            data_dir: PathBuf::from("/nonexistent"),
            output: None,
        };

        assert!(matches!(
            tool.fetch(request).await,
            Err(AcquireError::InvalidRequest(_))
        ));
        assert!(matches!(
            tool.probe("--exec=rm").await,
            Err(AcquireError::InvalidRequest(_))
        ));
        assert!(matches!(
            tool.doctor(Some("--exec=rm")).await,
            Err(AcquireError::InvalidRequest(_))
        ));
    }

    #[tokio::test]
    async fn doctor_works_without_a_stream() {
        let report = acquisition().doctor(None).await.unwrap();

        assert_eq!(report.policy, IpPolicy::V4Only);
        assert!(report.resolution.is_empty());
    }
}

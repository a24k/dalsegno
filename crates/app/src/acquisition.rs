//! The use cases the CLI calls in process, and the API will wrap at milestone 5 (D-17).
//!
//! Text input is validated here, before any external call.

use crate::backfill::{BackfillReport, backfill};
use crate::doctor::{DoctorReport, run_doctor};
use crate::error::AcquireError;
use crate::family::{FamilyController, SharedFamily};
use crate::features::{FeatureReport, extract_features};
use crate::fetch::{
    FetchLimits, FetchRange, FetchReport, Fetcher, Ledger, RetryPolicy, SharedDatabase,
    fetch_range, lock,
};
use crate::follow::{FollowEnd, FollowPolicy, follow};
use crate::monitor::{MonitorReport, run_pass};
use crate::probe::{ProbeReport, ProbeTools, probe_stream};
use crate::retention::{SweepReport, sweep};
use crate::session::{Clock, Session, system_clock};
pub use dalsegno_core::stream::IpPolicy;

use dalsegno_core::stream::VideoId;
use dalsegno_io::fragments::FragmentStore;
use dalsegno_io::http::{BindAddresses, FragmentClient};
use dalsegno_io::tools::{find_program, has_ipv6_route};
use dalsegno_io::ytdlp::YtDlp;
use dalsegno_store::database::Database;
use dalsegno_store::streams::{NewStream, StreamRow};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tokio::task::spawn_blocking;

const YT_DLP: &str = "yt-dlp";
const DATABASE_FILE: &str = "dalsegno.sqlite3";
/// Local files of exports in progress, under the data directory.
const WORK_DIRECTORY: &str = "work";
const MILLIS_PER_SECOND: i64 = 1_000;

/// Settings shared by the acquisition use cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcquisitionOptions {
    /// IP family policy.
    pub policy: IpPolicy,
    /// Cookie file for yt-dlp.
    pub cookies: Option<PathBuf>,
}

/// The database and fragment files under one data directory.
#[derive(Debug, Clone)]
pub struct Library {
    pub(crate) database: SharedDatabase,
    pub(crate) store: FragmentStore,
    pub(crate) work: PathBuf,
}

impl Library {
    /// Runs one pass of analysis over a stream.
    pub async fn analyse(&self, stream: &str) -> Result<MonitorReport, AcquireError> {
        let row = self.registered(stream)?;
        let database = Arc::clone(&self.database);
        let now = system_clock()() as i64 * MILLIS_PER_SECOND;
        spawn_blocking(move || run_pass(&database, &row, now))
            .await
            .map_err(|_| AcquireError::Task)?
    }

    /// Returns the database, for the service's own work.
    pub fn database(&self) -> SharedDatabase {
        Arc::clone(&self.database)
    }

    /// Opens or creates the library.
    pub fn open(data_dir: &Path) -> Result<Self, AcquireError> {
        fs::create_dir_all(data_dir)?;
        let database = Database::open(&data_dir.join(DATABASE_FILE))?;
        Ok(Self {
            database: Arc::new(Mutex::new(database)),
            store: FragmentStore::new(data_dir),
            work: data_dir.join(WORK_DIRECTORY),
        })
    }

    pub(crate) fn registered(&self, input: &str) -> Result<StreamRow, AcquireError> {
        let video = parse_stream(input)?;
        lock(&self.database)?
            .stream_by_video_id(video.as_str())?
            .ok_or_else(|| AcquireError::NotRegistered(video.to_string()))
    }

    /// Brings a stream's envelope up to date.
    pub async fn features(&self, stream: &str) -> Result<FeatureReport, AcquireError> {
        let row = self.registered(stream)?;
        let library = self.clone();
        spawn_blocking(move || {
            let mut database = lock(&library.database)?;
            extract_features(&mut database, &library.store, &row)
        })
        .await
        .map_err(|_| AcquireError::Task)?
    }

    /// Removes what retention allows.
    pub async fn sweep(&self) -> Result<SweepReport, AcquireError> {
        let library = self.clone();
        let now = system_clock()() as i64 * MILLIS_PER_SECOND;
        spawn_blocking(move || sweep(&mut *lock(&library.database)?, &library.store, now))
            .await
            .map_err(|_| AcquireError::Task)?
    }
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
    /// Where to join the range, once it is complete.
    pub output: Option<PathBuf>,
}

/// What registering a stream produced.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AddReport {
    /// Row ID of the stream.
    pub stream_id: i64,
    /// What the probe learned.
    pub probe: ProbeReport,
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

    /// Probes a stream and stores it in the `registered` state (FR-S1, FR-S2).
    pub async fn add(&self, library: &Library, stream: &str) -> Result<AddReport, AcquireError> {
        let video = parse_stream(stream)?;
        let probe = self.probe(stream).await?;
        let audio = probe.audio.as_ref().ok_or(AcquireError::NoAudio)?;
        let fragment_len = probe.fragment_seconds.ok_or(AcquireError::NoAudio)?;
        let row = NewStream {
            video_id: video.to_string(),
            title: probe.title.clone(),
            channel_id: probe.channel_id.clone(),
            channel_name: probe.channel.clone(),
            thumbnail_url: probe.thumbnail.clone(),
            video_itag: probe.video.as_ref().map(|format| format.itag),
            audio_itag: audio.itag,
            fragment_len,
            registered_at: system_clock()() as i64 * MILLIS_PER_SECOND,
        };
        let stream_id = lock(&library.database)?.insert_stream(&row)?;
        Ok(AddReport { stream_id, probe })
    }

    /// Fetches a range, and joins it when it is complete and an output is set.
    pub async fn fetch(
        &self,
        library: &Library,
        request: FetchRequest,
    ) -> Result<FetchReport, AcquireError> {
        let row = library.registered(&request.stream)?;
        let fetcher = Arc::new(self.fetcher(library, &row, request.itag)?);
        let range = FetchRange {
            stream_id: row.id,
            itag: request.itag,
            first: request.first,
            last: request.last,
        };
        let report = fetch_range(fetcher, range).await?;
        if let Some(output) = request.output.filter(|_| report.is_complete()) {
            let store = library.store.clone();
            let (id, itag, first, last) = (row.id, request.itag, request.first, request.last);
            spawn_blocking(move || store.concat(id, itag, first, last, &output))
                .await
                .map_err(|_| AcquireError::Task)??;
        }
        Ok(report)
    }

    /// Fills the last `hours` of a stream's audio.
    pub async fn backfill(
        &self,
        library: &Library,
        stream: &str,
        hours: f64,
    ) -> Result<BackfillReport, AcquireError> {
        let row = library.registered(stream)?;
        let fetcher = self.fetcher(library, &row, row.stream.audio_itag)?;
        backfill(Arc::new(fetcher), row.stream.fragment_len, hours).await
    }

    /// Follows a stream's audio until `stop` turns true, the stream ends, or following fails.
    pub async fn follow(
        &self,
        library: &Library,
        row: &StreamRow,
        stop: watch::Receiver<bool>,
    ) -> Result<FollowEnd, AcquireError> {
        let fetcher = self.fetcher(library, row, row.stream.audio_itag)?;
        let policy = FollowPolicy::for_fragment_length(row.stream.fragment_len);
        follow(Arc::new(fetcher), policy, stop).await
    }

    pub(crate) fn fetcher(
        &self,
        library: &Library,
        row: &StreamRow,
        itag: u32,
    ) -> Result<Fetcher<YtDlp>, AcquireError> {
        let video = parse_stream(&row.stream.video_id)?;
        let clock: Clock = system_clock();
        Ok(Fetcher {
            session: Session::new(
                Arc::clone(&self.resolver),
                video,
                itag,
                Arc::clone(&self.family),
                Arc::clone(&clock),
                RetryPolicy::default().base_delay,
            ),
            client: self.client.clone(),
            store: library.store.clone(),
            limits: FetchLimits::default(),
            retry: RetryPolicy::default(),
            ledger: Ledger {
                database: Arc::clone(&library.database),
                stream_id: row.id,
                itag,
                fragment_len: row.stream.fragment_len,
            },
            clock,
        })
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
            output: None,
        };
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(data.path()).unwrap();

        assert!(matches!(
            tool.fetch(&library, request).await,
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
    async fn streams_must_be_registered_before_use() {
        let data = tempfile::tempdir().unwrap();
        let library = Library::open(data.path()).unwrap();

        assert!(matches!(
            library.features("abcDEF123_-").await,
            Err(AcquireError::NotRegistered(_))
        ));
        assert!(matches!(
            acquisition().backfill(&library, "abcDEF123_-", 1.0).await,
            Err(AcquireError::NotRegistered(_))
        ));
    }

    #[tokio::test]
    async fn sweeping_an_empty_library_does_nothing() {
        let data = tempfile::tempdir().unwrap();

        let report = Library::open(data.path()).unwrap().sweep().await.unwrap();

        assert_eq!(report, SweepReport::default());
    }

    #[tokio::test]
    async fn doctor_works_without_a_stream() {
        let report = acquisition().doctor(None).await.unwrap();

        assert_eq!(report.policy, IpPolicy::V4Only);
        assert!(report.resolution.is_empty());
    }
}

//! What the boundary editor needs for a draft clip: per edge a preview, a contact sheet and
//! the candidates around it, and for a cycle its measured period (FR-E2, FR-E3, FR-E4).
//!
//! Each edge has a folder of work files, and `report.json` is written last, so its presence
//! means the others are in place.

use super::candidates::{Ensure, MARGIN_SECONDS, examine, neighbourhood, refine_with};
use crate::acquisition::{Acquisition, Library};
use crate::error::AcquireError;
use crate::fetch::lock;
use dalsegno_core::export::{EndCandidate, Interval, Keyframe, RefinedPeriod, StartCandidate};
use dalsegno_io::ffmpeg::Ffmpeg;
use dalsegno_store::clips::{ClipKind, ClipState, ClipStatus};
use dalsegno_store::jobs::{JobClass, NewJob};
use dalsegno_store::streams::StreamRow;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tokio::task::spawn_blocking;

/// The job that prepares a draft clip's boundary candidates.
pub const CANDIDATES: &str = "candidates";
/// The job centres its neighbourhood on the edge and looks `MARGIN_SECONDS` to each side. A
/// report serves the edge while it stays within that reach less this many seconds, as there is
/// too little video left around it beyond.
pub const STALE_MARGIN_SECONDS: f64 = 5.0;
/// A period measured from a start is stale once the start moves by more than this, in seconds.
pub const PERIOD_TOLERANCE_SECONDS: f64 = 0.001;

const CLIPS_DIRECTORY: &str = "clips";
const PERIOD_FILE: &str = "period.json";

/// One end of a cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// Where the cut begins.
    Start,
    /// Where the cut ends.
    End,
}

impl Edge {
    /// Both edges, start first.
    pub const ALL: [Self; 2] = [Self::Start, Self::End];

    /// The name used in paths and in the API.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::End => "end",
        }
    }

    /// Reads a name.
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|edge| edge.as_str() == text)
    }

    fn time(self, clip: &ClipStatus) -> Option<f64> {
        match self {
            Self::Start => clip.start,
            Self::End => clip.end,
        }
    }
}

/// A file kept for one edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeFile {
    /// The candidates and what they rest on.
    Report,
    /// The video to play.
    Preview,
    /// Frames of the window, to choose by eye.
    Sheet,
}

impl EdgeFile {
    fn name(self) -> &'static str {
        match self {
            Self::Report => "report.json",
            Self::Preview => "preview.mp4",
            Self::Sheet => "sheet.jpg",
        }
    }
}

/// The clip a candidates job is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidatesTarget {
    /// Clip row ID.
    pub clip: i64,
}

impl CandidatesTarget {
    /// Returns the job.
    pub fn job(&self) -> Result<NewJob, AcquireError> {
        Ok(NewJob {
            kind: CANDIDATES.to_string(),
            class: JobClass::Ffmpeg,
            target: serde_json::to_string(self).map_err(|_| AcquireError::Task)?,
        })
    }

    /// Reads a stored target.
    pub fn parse(target: &str) -> Option<Self> {
        serde_json::from_str(target).ok()
    }
}

/// The stretch of stream time that an edge's work files cover.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Window {
    /// Where it starts.
    pub start: f64,
    /// Where it ends.
    pub end: f64,
}

/// Everything found around one edge, in stream time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeReport {
    /// The time examined.
    pub around: f64,
    /// What the work files cover.
    pub window: Window,
    /// Stream time of the preview's first frame.
    pub preview_start: f64,
    /// Fragment starts in the window.
    pub keyframes: Vec<Keyframe>,
    /// Where the picture is black.
    pub black: Vec<Interval>,
    /// Where the sound is silent.
    pub silence: Vec<Interval>,
    /// Where a cut can start, best first.
    pub starts: Vec<StartCandidate>,
    /// Where a cut can end, best first.
    pub ends: Vec<EndCandidate>,
    /// Whether silence had to be loosened to find anything.
    pub relaxed: bool,
}

/// The period of a cycle clip, measured from the clip's start (FR-E3).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PeriodReport {
    /// The start it was measured from.
    pub start: f64,
    /// The period from audio.
    pub audio: f64,
    /// The period from black, when two cycles agree.
    pub video: Option<RefinedPeriod>,
}

/// What the editor shows for a clip: only what is current for the clip as it stands.
#[derive(Debug, Clone, PartialEq)]
pub struct EditorState {
    /// The clip.
    pub clip: ClipStatus,
    /// The start's report, if current.
    pub start: Option<EdgeReport>,
    /// The end's report, if current.
    pub end: Option<EdgeReport>,
    /// The measured period, if current.
    pub period: Option<PeriodReport>,
    /// Whether a draft lacks something that the candidates job would make.
    pub needs_job: bool,
}

/// Whether a report still serves an edge at `time`: the edge lies within the reach of the time
/// the report was made around. The job centres the neighbourhood on the edge, so a report stays
/// current even where the stored data, and so its window, ends.
pub fn is_edge_current(report: Option<&EdgeReport>, time: f64) -> bool {
    report
        .is_some_and(|report| (time - report.around).abs() <= MARGIN_SECONDS - STALE_MARGIN_SECONDS)
}

/// Whether a period was measured from this start.
pub fn is_period_current(period: Option<&PeriodReport>, start: f64) -> bool {
    period.is_some_and(|period| (period.start - start).abs() <= PERIOD_TOLERANCE_SECONDS)
}

/// The start a clip's period is measured from, for a cycle that cuts a version.
fn period_start(clip: &ClipStatus) -> Option<f64> {
    let is_measured = clip.kind == ClipKind::Cycle && clip.version_id.is_some();
    clip.start.filter(|_| is_measured)
}

/// Reads a JSON file this program wrote. One that is missing or unreadable reads as absent.
fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

/// Writes a file under a temporary name and renames it, so a reader never sees half of it.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let partial = path.with_extension("partial");
    fs::write(&partial, bytes)?;
    fs::rename(&partial, path).inspect_err(|_| {
        let _ = fs::remove_file(&partial);
    })
}

/// Runs file work on a blocking thread.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> Result<T, AcquireError> {
    Ok(spawn_blocking(work)
        .await
        .map_err(|_| AcquireError::Task)??)
}

impl Library {
    /// The folder of one clip's work files.
    pub fn clip_work(&self, clip: i64) -> PathBuf {
        self.work.join(CLIPS_DIRECTORY).join(clip.to_string())
    }

    /// The folder of one edge's work files.
    pub fn edge_directory(&self, clip: i64, edge: Edge) -> PathBuf {
        self.clip_work(clip).join(edge.as_str())
    }

    fn edge_report(&self, clip: i64, edge: Edge) -> Option<EdgeReport> {
        read_json(
            &self
                .edge_directory(clip, edge)
                .join(EdgeFile::Report.name()),
        )
    }

    fn period_report(&self, clip: i64) -> Option<PeriodReport> {
        read_json(&self.clip_work(clip).join(PERIOD_FILE))
    }

    fn clip_status(&self, clip: i64) -> Result<ClipStatus, AcquireError> {
        lock(&self.database)?
            .clip_status(clip)?
            .ok_or(AcquireError::NotFound("clip"))
    }

    /// Returns the path of a file of an edge's current work, which the clip's edge still lies
    /// in. A clip without it, or with it out of date, is not found.
    pub fn current_edge_file(
        &self,
        clip: i64,
        edge: Edge,
        file: EdgeFile,
    ) -> Result<PathBuf, AcquireError> {
        let status = self.clip_status(clip)?;
        let time = edge.time(&status).ok_or(AcquireError::NotFound("file"))?;
        if !is_edge_current(self.edge_report(clip, edge).as_ref(), time) {
            return Err(AcquireError::NotFound("file"));
        }
        let path = self.edge_directory(clip, edge).join(file.name());
        if path.is_file() {
            Ok(path)
        } else {
            Err(AcquireError::NotFound("file"))
        }
    }
}

/// Reads a clip with the reports that are current for it, and says whether the candidates job
/// has anything to make. An exported clip never does.
pub fn editor_state(library: &Library, clip_id: i64) -> Result<EditorState, AcquireError> {
    let clip = library.clip_status(clip_id)?;
    let current = |edge: Edge| {
        let time = edge.time(&clip)?;
        let report = library.edge_report(clip.id, edge);
        is_edge_current(report.as_ref(), time)
            .then_some(report)
            .flatten()
    };
    let (start, end) = (current(Edge::Start), current(Edge::End));
    let period = library.period_report(clip.id);
    let period = period.filter(|period| is_period_current(Some(period), clip.start.unwrap_or(0.0)));
    let lacks_edge =
        |edge: Edge, report: &Option<EdgeReport>| edge.time(&clip).is_some() && report.is_none();
    let lacks_period = period_start(&clip).is_some() && period.is_none();
    let needs_job = clip.state == ClipState::Draft
        && (lacks_edge(Edge::Start, &start) || lacks_edge(Edge::End, &end) || lacks_period);
    Ok(EditorState {
        clip,
        start,
        end,
        period,
        needs_job,
    })
}

impl Library {
    /// Makes one edge's work files, with `report.json` last. The old report goes first, so a
    /// reader never pairs it with new files.
    async fn prepare_edge<E: Ensure>(
        &self,
        tools: &Ffmpeg,
        row: &StreamRow,
        clip: i64,
        edge: Edge,
        time: f64,
        ensure: &E,
    ) -> Result<(), AcquireError> {
        let directory = self.edge_directory(clip, edge);
        let report_path = directory.join(EdgeFile::Report.name());
        let (made, old) = (directory.clone(), report_path.clone());
        blocking(move || {
            fs::create_dir_all(&made)?;
            match fs::remove_file(&old) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            }
        })
        .await?;
        let place = neighbourhood(self, row, time, ensure).await?;
        let found = examine(tools, &place).await?;
        tools
            .preview(
                &place.video,
                &place.audio,
                &directory.join(EdgeFile::Preview.name()),
            )
            .await?;
        tools
            .contact_sheet(&place.video, &directory.join(EdgeFile::Sheet.name()))
            .await?;
        let report = EdgeReport {
            around: time,
            window: Window {
                start: place.video_start(),
                end: place.end(),
            },
            preview_start: place.video_start(),
            keyframes: place.keyframes(),
            black: found.black,
            silence: found.silence,
            starts: found.starts,
            ends: found.ends,
            relaxed: found.relaxed,
        };
        let bytes = serde_json::to_vec(&report).map_err(|_| AcquireError::Task)?;
        blocking(move || write_atomic(&report_path, &bytes)).await
    }

    /// Measures the period of a cycle from its start. A start with no audio stored one cycle
    /// later gives a period with no video measure.
    async fn prepare_period<E: Ensure>(
        &self,
        tools: &Ffmpeg,
        row: &StreamRow,
        clip: i64,
        start: f64,
        audio: f64,
        ensure: &E,
    ) -> Result<(), AcquireError> {
        let video = match refine_with(self, tools, row, start, audio, ensure).await {
            Ok(report) => report.video,
            Err(AcquireError::InvalidRequest(_)) => None,
            Err(error) => return Err(error),
        };
        let report = PeriodReport {
            start,
            audio,
            video,
        };
        let bytes = serde_json::to_vec(&report).map_err(|_| AcquireError::Task)?;
        let directory = self.clip_work(clip);
        blocking(move || {
            fs::create_dir_all(&directory)?;
            write_atomic(&directory.join(PERIOD_FILE), &bytes)
        })
        .await
    }
}

/// Makes what a draft's editor lacks, from fragments `ensure` brings in. An exported clip has
/// nothing to make.
pub(crate) async fn prepare_with<E: Ensure>(
    library: &Library,
    tools: &Ffmpeg,
    clip_id: i64,
    ensure: E,
) -> Result<(), AcquireError> {
    let clip = library.clip_status(clip_id)?;
    if clip.state != ClipState::Draft {
        return Ok(());
    }
    let row = lock(&library.database)?
        .stream(clip.stream_id)?
        .ok_or(AcquireError::NotFound("stream"))?;
    for edge in Edge::ALL {
        let Some(time) = edge.time(&clip) else {
            continue;
        };
        let stored = library.edge_report(clip.id, edge);
        if !is_edge_current(stored.as_ref(), time) {
            library
                .prepare_edge(tools, &row, clip.id, edge, time, &ensure)
                .await?;
        }
    }
    let Some(start) = period_start(&clip) else {
        return Ok(());
    };
    if is_period_current(library.period_report(clip.id).as_ref(), start) {
        return Ok(());
    }
    let version = clip.version_id.unwrap_or_default();
    let audio = lock(&library.database)?
        .version(version)?
        .ok_or(AcquireError::NotFound("version"))?
        .period;
    library
        .prepare_period(tools, &row, clip.id, start, audio, &ensure)
        .await
}

/// Fetches what is missing from the origin.
struct Fetching<'a>(&'a Acquisition, &'a Library);

impl Ensure for Fetching<'_> {
    async fn ensure(
        &self,
        row: &StreamRow,
        itag: u32,
        first: u64,
        last: u64,
    ) -> Result<(), AcquireError> {
        self.0.ensure_range(self.1, row, itag, first, last).await
    }
}

impl Acquisition {
    /// Makes the preview, sheet and candidates of both edges of a draft, and the period of a
    /// cycle. Edges that are current are left as they are, so it is safe to run again.
    pub async fn prepare_editor(
        &self,
        library: &Library,
        tools: &Ffmpeg,
        clip_id: i64,
    ) -> Result<(), AcquireError> {
        prepare_with(library, tools, clip_id, Fetching(self, library)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::candidates::Using;
    use crate::export::find_tools;
    use dalsegno_store::fragments::{FragmentRow, HeadSample};
    use dalsegno_store::streams::NewStream;
    use std::process::Command;
    use tempfile::{TempDir, tempdir};

    const VIDEO_ITAG: u32 = 248;
    const AUDIO_ITAG: u32 = 140;
    const STREAM_TIME: f64 = 1_000.0;
    const SECONDS: f64 = 20.0;

    struct Setup {
        _root: TempDir,
        library: Library,
        tools: Ffmpeg,
    }

    fn generate(ffmpeg: &Path, args: &[&str], output: &Path) -> Vec<u8> {
        let status = Command::new(ffmpeg)
            .args(["-v", "error", "-y"])
            .args(args)
            .arg(output)
            .status()
            .unwrap();
        assert!(status.success());
        fs::read(output).unwrap()
    }

    /// A stream with one 20 s fragment of each track, which is all the library holds. The head
    /// sits one past it, so the job asks for no fragment that is missing.
    fn setup() -> Option<Setup> {
        let tools = find_tools().ok()?;
        let root = tempdir().unwrap();
        let library = Library::open(&root.path().join("data")).unwrap();
        let database = lock(&library.database).unwrap();
        let id = database
            .insert_stream(&NewStream {
                video_id: "abcDEF123_-".to_string(),
                title: "Example TV".to_string(),
                channel_id: None,
                channel_name: None,
                thumbnail_url: None,
                video_itag: Some(VIDEO_ITAG),
                audio_itag: AUDIO_ITAG,
                fragment_len: SECONDS,
                registered_at: 0,
            })
            .unwrap();
        let video = generate(
            &tools.ffmpeg,
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc=s=64x64:r=10:d=20",
                "-c:v",
                "mpeg4",
            ],
            &root.path().join("v.mkv"),
        );
        let audio = generate(
            &tools.ffmpeg,
            &["-f", "lavfi", "-i", "sine=f=440:d=20", "-c:a", "aac"],
            &root.path().join("a.m4a"),
        );
        for (itag, bytes) in [(VIDEO_ITAG, &video), (AUDIO_ITAG, &audio)] {
            library.store.write(id, itag, 0, bytes).unwrap();
            database
                .put_fragment(&FragmentRow {
                    stream_id: id,
                    itag,
                    sq: 0,
                    stream_time: STREAM_TIME,
                    duration: SECONDS,
                    size: bytes.len() as u64,
                    fetched_at: 0,
                })
                .unwrap();
        }
        database
            .put_head_sample(&HeadSample {
                stream_id: id,
                observed_at: 0,
                head_sq: 1,
                head_time: STREAM_TIME + SECONDS,
            })
            .unwrap();
        drop(database);
        Some(Setup {
            _root: root,
            library,
            tools,
        })
    }

    fn insert_draft(setup: &Setup, start: f64, end: f64) -> i64 {
        let database = lock(&setup.library.database).unwrap();
        database
            .connection()
            .execute(
                "INSERT INTO clips (state, kind, stream_id, start_time, end_time)
                 VALUES ('draft', 'addition', 1, ?1, ?2)",
                [start, end],
            )
            .unwrap();
        database.connection().last_insert_rowid()
    }

    async fn prepare(setup: &Setup, clip: i64) -> Result<(), AcquireError> {
        let ensure = Using(async |_: &StreamRow, _, _, _| Ok(()));
        prepare_with(&setup.library, &setup.tools, clip, ensure).await
    }

    #[tokio::test]
    async fn the_job_makes_every_file_of_both_edges_and_the_state_is_then_current() {
        let Some(setup) = setup() else {
            return;
        };
        let clip = insert_draft(&setup, 1_006.0, 1_012.0);
        assert!(editor_state(&setup.library, clip).unwrap().needs_job);

        prepare(&setup, clip).await.unwrap();

        for edge in Edge::ALL {
            for file in [EdgeFile::Report, EdgeFile::Preview, EdgeFile::Sheet] {
                let path = setup.library.current_edge_file(clip, edge, file).unwrap();
                assert!(path.is_file(), "{edge:?} {file:?}");
            }
        }
        let state = editor_state(&setup.library, clip).unwrap();
        let start = state.start.unwrap();
        assert_eq!(
            (state.needs_job, start.around, start.window.start),
            (false, 1_006.0, STREAM_TIME)
        );
        assert_eq!(start.keyframes.len(), 1);
        assert!(state.end.is_some());
    }

    #[tokio::test]
    async fn an_edge_moved_far_from_where_it_was_examined_is_redone_and_the_other_is_kept() {
        let Some(setup) = setup() else {
            return;
        };
        let clip = insert_draft(&setup, 1_006.0, 1_012.0);
        prepare(&setup, clip).await.unwrap();
        let kept = fs::read(
            setup
                .library
                .edge_directory(clip, Edge::End)
                .join("report.json"),
        )
        .unwrap();
        lock(&setup.library.database)
            .unwrap()
            .connection()
            .execute(
                "UPDATE clips SET start_time = 1_025.0 WHERE id = ?1",
                [clip],
            )
            .unwrap();

        let stale = editor_state(&setup.library, clip).unwrap();
        prepare(&setup, clip).await.unwrap();

        assert_eq!((stale.start.is_none(), stale.end.is_some()), (true, true));
        assert!(stale.needs_job);
        let redone: EdgeReport = read_json(
            &setup
                .library
                .edge_directory(clip, Edge::Start)
                .join("report.json"),
        )
        .unwrap();
        assert_eq!(redone.around, 1_025.0);
        let end = fs::read(
            setup
                .library
                .edge_directory(clip, Edge::End)
                .join("report.json"),
        )
        .unwrap();
        assert_eq!(end, kept);
    }

    #[tokio::test]
    async fn an_exported_clip_is_left_alone() {
        let Some(setup) = setup() else {
            return;
        };
        let clip = insert_draft(&setup, 1_006.0, 1_012.0);
        lock(&setup.library.database)
            .unwrap()
            .connection()
            .execute(
                "UPDATE clips SET state = 'exported', name = 'Example', video_itag = 248,
                 audio_itag = 140, sq_first = 0, sq_last = 0, duration = 6.0, metadata = '{}'",
                [],
            )
            .unwrap();

        prepare(&setup, clip).await.unwrap();

        let state = editor_state(&setup.library, clip).unwrap();
        assert!(!state.needs_job);
        assert!(!setup.library.clip_work(clip).exists());
    }

    #[test]
    fn an_unknown_clip_is_not_found() {
        let root = tempdir().unwrap();
        let library = Library::open(root.path()).unwrap();

        let result = editor_state(&library, 99);

        assert!(matches!(result, Err(AcquireError::NotFound("clip"))));
    }

    fn report(start: f64, end: f64) -> EdgeReport {
        EdgeReport {
            around: (start + end) / 2.0,
            window: Window { start, end },
            preview_start: start,
            keyframes: Vec::new(),
            black: Vec::new(),
            silence: Vec::new(),
            starts: Vec::new(),
            ends: Vec::new(),
            relaxed: false,
        }
    }

    #[test]
    fn an_edge_well_inside_the_window_is_current() {
        let stored = report(1_000.0, 1_040.0);

        assert!(is_edge_current(Some(&stored), 1_020.0));
        assert!(is_edge_current(Some(&stored), 1_005.0));
        assert!(is_edge_current(Some(&stored), 1_035.0));
    }

    #[test]
    fn an_edge_far_from_where_it_was_examined_is_stale() {
        let stored = report(1_000.0, 1_040.0);

        for time in [1_004.9, 1_035.1, 900.0, 2_000.0] {
            assert!(!is_edge_current(Some(&stored), time), "{time}");
        }
    }

    #[test]
    fn an_edge_at_the_end_of_the_data_is_current_when_it_is_where_it_was_examined() {
        let stored = EdgeReport {
            around: 1_040.0,
            ..report(1_000.0, 1_040.0)
        };

        assert!(is_edge_current(Some(&stored), 1_040.0));
        assert!(is_edge_current(Some(&stored), 1_055.0));
        assert!(!is_edge_current(Some(&stored), 1_055.1));
    }

    #[test]
    fn a_missing_report_is_stale() {
        assert!(!is_edge_current(None, 1_020.0));
    }

    #[test]
    fn a_period_is_current_for_the_start_it_was_measured_from() {
        let period = PeriodReport {
            start: 500.0,
            audio: 3_600.0,
            video: None,
        };

        assert!(is_period_current(Some(&period), 500.0005));
        assert!(!is_period_current(Some(&period), 500.01));
        assert!(!is_period_current(None, 500.0));
    }

    #[test]
    fn edges_are_named_as_their_folders_are() {
        assert_eq!(Edge::parse("start"), Some(Edge::Start));
        assert_eq!(Edge::parse("end"), Some(Edge::End));
        assert_eq!(Edge::parse("middle"), None);
        assert_eq!(Edge::End.as_str(), "end");
    }

    #[test]
    fn the_target_survives_the_job_row() {
        let target = CandidatesTarget { clip: 7 };

        let job = target.job().unwrap();

        assert_eq!(
            (job.kind.as_str(), job.class),
            (CANDIDATES, JobClass::Ffmpeg)
        );
        assert_eq!(job.target, r#"{"clip":7}"#);
        assert_eq!(CandidatesTarget::parse(&job.target), Some(target));
        assert_eq!(CandidatesTarget::parse("{}"), None);
    }

    #[test]
    fn a_file_is_written_whole_or_not_at_all() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.json");

        write_atomic(&path, b"{}").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"{}");
        assert!(!path.with_extension("partial").exists());
    }

    #[test]
    fn an_unreadable_report_reads_as_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.json");
        fs::write(&path, b"not json").unwrap();

        assert_eq!(read_json::<EdgeReport>(&path), None);
        assert_eq!(read_json::<EdgeReport>(&dir.path().join("none.json")), None);
    }
}

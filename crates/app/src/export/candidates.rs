//! Boundary candidates and the frame-accurate period (FR-E2, FR-E3, FR-E4).

use crate::acquisition::{Acquisition, Library};
use crate::error::AcquireError;
use crate::fetch::lock;
use dalsegno_core::export::{
    EndCandidate, Interval, Keyframe, RefinedPeriod, StartCandidate, end_candidates, parse_black,
    parse_silence, refine_period, start_candidates,
};
use dalsegno_io::ffmpeg::{Ffmpeg, SilenceLevel};
use dalsegno_store::fragments::FragmentRow;
use dalsegno_store::streams::StreamRow;
use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use tokio::task::spawn_blocking;

/// How far either side of a boundary is examined, in seconds.
const MARGIN_SECONDS: f64 = 20.0;

/// What a neighbourhood of a boundary holds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CandidateReport {
    /// The time examined.
    pub around: f64,
    /// Where a cut can start, best first.
    pub starts: Vec<StartCandidate>,
    /// Where a cut can end, best first.
    pub ends: Vec<EndCandidate>,
    /// Whether silence had to be loosened to find anything.
    pub relaxed: bool,
    /// Frames of the neighbourhood, for choosing by eye.
    pub contact_sheet: PathBuf,
}

/// The frame-accurate period, beside the audio estimate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct RefineReport {
    /// The period from audio.
    pub audio: f64,
    /// The period from black, when two cycles agree.
    pub video: Option<RefinedPeriod>,
}

struct Neighbourhood {
    video: PathBuf,
    audio: PathBuf,
    video_rows: Vec<FragmentRow>,
    audio_start: f64,
}

impl Neighbourhood {
    fn video_start(&self) -> f64 {
        self.video_rows.first().map_or(0.0, |row| row.stream_time)
    }

    fn end(&self) -> f64 {
        self.video_rows
            .last()
            .map_or(0.0, |row| row.stream_time + row.duration)
    }

    fn keyframes(&self) -> Vec<Keyframe> {
        self.video_rows
            .iter()
            .map(|row| Keyframe {
                sq: row.sq,
                time: row.stream_time,
            })
            .collect()
    }
}

fn shifted(intervals: &[Interval], by: f64) -> Vec<Interval> {
    intervals
        .iter()
        .map(|interval| Interval {
            start: interval.start + by,
            end: interval.end + by,
        })
        .collect()
}

/// Brings in both tracks around `time` through `ensure`, and joins them into work files.
async fn neighbourhood<F>(
    library: &Library,
    row: &StreamRow,
    time: f64,
    ensure: &F,
) -> Result<Neighbourhood, AcquireError>
where
    F: AsyncFn(&StreamRow, u32, u64, u64) -> Result<(), AcquireError>,
{
    let video_itag = row
        .stream
        .video_itag
        .ok_or_else(|| AcquireError::NoVideo(row.stream.video_id.clone()))?;
    let audio_itag = row.stream.audio_itag;
    let centre = lock(&library.database)?
        .fragment_at_time(row.id, audio_itag, time)?
        .ok_or(AcquireError::InvalidRequest(
            "no stored audio places this time, run backfill first",
        ))?;
    let reach = (MARGIN_SECONDS / row.stream.fragment_len).ceil() as u64;
    // The newest fragment is still being written, so the range stops before the head.
    let head = lock(&library.database)?
        .latest_head_sample(row.id)?
        .map(|sample| sample.head_sq.saturating_sub(1));
    let first = centre.saturating_sub(reach);
    let last = head.map_or(centre + reach, |sq| (centre + reach).min(sq.max(centre)));
    ensure(row, video_itag, first, last).await?;
    ensure(row, audio_itag, first, last).await?;
    let (video_rows, audio_rows) = {
        let database = lock(&library.database)?;
        (
            database.fragments_in_range(row.id, video_itag, first, last)?,
            database.fragments_in_range(row.id, audio_itag, first, last)?,
        )
    };
    let directory = library.work.join(format!("around-{}-{first}", row.id));
    let video = directory.join("video.webm");
    let audio = directory.join("audio.m4a");
    let store = library.store.clone();
    let id = row.id;
    let (joined_video, joined_audio) = (video.clone(), audio.clone());
    spawn_blocking(move || -> std::io::Result<()> {
        fs::create_dir_all(&directory)?;
        store.concat(id, video_itag, first, last, &joined_video)?;
        store.concat(id, audio_itag, first, last, &joined_audio)
    })
    .await
    .map_err(|_| AcquireError::Task)??;
    let audio_start = audio_rows.first().map_or(0.0, |row| row.stream_time);
    Ok(Neighbourhood {
        video,
        audio,
        video_rows,
        audio_start,
    })
}

/// Lists where a cut near `time` could start and end, from fragments `ensure` brings in.
pub(crate) async fn candidates_with<F>(
    library: &Library,
    tools: &Ffmpeg,
    stream: &str,
    time: f64,
    ensure: F,
) -> Result<CandidateReport, AcquireError>
where
    F: AsyncFn(&StreamRow, u32, u64, u64) -> Result<(), AcquireError>,
{
    let row = library.registered(stream)?;
    let place = neighbourhood(library, &row, time, &ensure).await?;
    let black_log = tools.detect_black(&place.video).await?;
    let black = shifted(&parse_black(&black_log), place.video_start());
    let length = place.end() - place.audio_start;
    let fine = tools
        .detect_silence(&place.audio, SilenceLevel::Fine)
        .await?;
    let found = parse_silence(&fine, length);
    let relaxed = found.is_empty();
    let silence = if relaxed {
        let log = tools
            .detect_silence(&place.audio, SilenceLevel::Relaxed)
            .await?;
        parse_silence(&log, length)
    } else {
        found
    };
    let silence = shifted(&silence, place.audio_start);
    let contact_sheet = place.video.with_file_name("sheet.jpg");
    tools.contact_sheet(&place.video, &contact_sheet).await?;
    Ok(CandidateReport {
        around: time,
        starts: start_candidates(&black, &silence, &place.keyframes()),
        ends: end_candidates(&black, &silence, place.audio_start + length),
        relaxed,
        contact_sheet,
    })
}

impl Acquisition {
    /// Lists where a cut near `time` could start and end. A person chooses (G3).
    pub async fn candidates(
        &self,
        library: &Library,
        tools: &Ffmpeg,
        stream: &str,
        time: f64,
    ) -> Result<CandidateReport, AcquireError> {
        let ensure = async |row: &StreamRow, itag, first, last| {
            self.ensure_range(library, row, itag, first, last).await
        };
        candidates_with(library, tools, stream, time, ensure).await
    }

    /// Measures the period from black at a boundary and one cycle later (FR-E3).
    pub async fn refine(
        &self,
        library: &Library,
        tools: &Ffmpeg,
        stream: &str,
        boundary: f64,
        period: f64,
    ) -> Result<RefineReport, AcquireError> {
        let row = library.registered(stream)?;
        let ensure = async |row: &StreamRow, itag, first, last| {
            self.ensure_range(library, row, itag, first, last).await
        };
        let mut black = Vec::with_capacity(2);
        for time in [boundary, boundary + period] {
            let place = neighbourhood(library, &row, time, &ensure).await?;
            let log = tools.detect_black(&place.video).await?;
            black.push(shifted(&parse_black(&log), place.video_start()));
        }
        Ok(RefineReport {
            audio: period,
            video: refine_period(&black[0], &black[1], period),
        })
    }
}

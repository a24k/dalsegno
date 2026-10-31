//! Making the finished folder of a cut (FR-E4 to FR-E9, FR-E12). Streams are copied, never
//! encoded (G4).

use crate::acquisition::{Acquisition, Library};
use crate::error::AcquireError;
use crate::fetch::lock;
use dalsegno_core::export::{
    CutError, ExportKind, ExportName, FragmentRange, NameParts, Piece, check_air_time, plan_cut,
    render_name, sanitise_info,
};
use dalsegno_io::ffmpeg::{Assembly, Ffmpeg, MediaFacts, Tags};
use dalsegno_store::clips::{ClipKind, ClipRow, NewClip};
use dalsegno_store::fragments::FragmentRow;
use dalsegno_store::programs::ProgramRow;
use dalsegno_store::streams::StreamRow;
use serde::Serialize;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use tokio::task::spawn_blocking;

/// A poster sits this far into an addition by default, past any title card.
const DEFAULT_POSTER_SECONDS: f64 = 90.0;
/// Fragments past the expected end, as fragments are not all one length. They are fetched only
/// when the cut needs them, as they may not exist yet.
const SPARE_FRAGMENTS: u64 = 2;
/// Longest cut, in seconds, far past any real cycle.
const MAX_CUT_SECONDS: f64 = 7.0 * 86_400.0;
/// Thumbnails come from here. Anything else in a stored URL is not followed.
const THUMBNAIL_HOST_SUFFIX: &str = ".ytimg.com";

/// The time a version was on air, in stream time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AirTime {
    /// When it began.
    pub from: f64,
    /// When it was superseded, if it was.
    pub until: Option<f64>,
}

/// Tags that replace the proposed ones (FR-E8).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TagEdits {
    /// `artist`.
    pub artist: Option<String>,
    /// `date`.
    pub date: Option<String>,
    /// `description`.
    pub description: Option<String>,
}

/// One cut.
#[derive(Debug, Clone, PartialEq)]
pub struct CutRequest {
    /// Video ID or URL.
    pub stream: String,
    /// What it holds.
    pub kind: ExportKind,
    /// The fragment whose start is the chosen keyframe.
    pub start_sq: u64,
    /// Length, in seconds. The end can be any frame.
    pub duration: f64,
    /// The moment the name refers to, in Unix seconds.
    pub named_at: i64,
    /// Checked for a full cycle (FR-E5).
    pub air_time: Option<AirTime>,
    /// Goes ahead with a cycle outside its air time, or with no air time given.
    pub override_air_time: bool,
    /// Replacements for the proposed tags.
    pub tags: TagEdits,
    /// Where a poster frame sits in an addition, in seconds from its start.
    pub poster_seconds: Option<f64>,
}

/// What a cut produced.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CutReport {
    /// Row ID of the clip.
    pub clip_id: i64,
    /// Output name.
    pub name: String,
    /// Finished folder in the work area.
    pub folder: PathBuf,
    /// Fragments used.
    pub range: FragmentRange,
    /// Length of the mp4, as ffprobe reads it.
    pub duration: f64,
}

fn pieces(rows: &[FragmentRow]) -> Vec<Piece> {
    rows.iter()
        .map(|row| Piece {
            sq: row.sq,
            time: row.stream_time,
            duration: row.duration,
        })
        .collect()
}

fn minutes_and_seconds(seconds: f64) -> String {
    let whole = seconds.round() as u64;
    format!("{}:{:02}", whole / 60, whole % 60)
}

/// Proposes the tags of a cut (FR-E8), which `edits` override.
fn proposed_tags(
    name: &ExportName,
    request: &CutRequest,
    program: &ProgramRow,
    row: &StreamRow,
) -> Tags {
    let program = &program.program;
    let description = match request.kind {
        ExportKind::Addition => format!(
            "Only the part added to the loop, {} long.",
            minutes_and_seconds(request.duration)
        ),
        ExportKind::FirstCycle | ExportKind::Cycle => {
            program.description.clone().unwrap_or_default()
        }
    };
    let edits = request.tags.clone();
    Tags {
        title: name.title.clone(),
        artist: edits
            .artist
            .or_else(|| program.artist.clone())
            .or_else(|| row.stream.channel_name.clone())
            .unwrap_or_default(),
        date: edits
            .date
            .or_else(|| program.content_date.clone())
            .unwrap_or_default(),
        description: edits.description.unwrap_or(description),
    }
}

fn check_air(request: &CutRequest, start: f64, end: f64) -> Result<(), AcquireError> {
    if request.kind == ExportKind::Addition || request.override_air_time {
        return Ok(());
    }
    let air = request.air_time.ok_or(AcquireError::InvalidRequest(
        "a cycle needs its air time, or an override",
    ))?;
    Ok(check_air_time(start, end, air.from, air.until)?)
}

fn expected_fragments(request: &CutRequest, row: &StreamRow) -> u64 {
    (request.duration / row.stream.fragment_len).ceil() as u64 - 1
}

fn is_thumbnail(url: &str) -> bool {
    let host = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split('/').next());
    host.is_some_and(|host| {
        host.ends_with(THUMBNAIL_HOST_SUFFIX)
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
    })
}

/// Keeps a poster inside a short addition.
fn poster_seconds(request: &CutRequest) -> f64 {
    request
        .poster_seconds
        .unwrap_or(DEFAULT_POSTER_SECONDS)
        .min(request.duration / 2.0)
}

/// Fails unless the mp4 is the cut that was asked for: length within one frame, both streams
/// there, and codecs unchanged.
fn verify(
    output: &MediaFacts,
    source_video: &MediaFacts,
    source_audio: &MediaFacts,
    duration: f64,
) -> Result<f64, AcquireError> {
    let found = output
        .duration
        .ok_or_else(|| AcquireError::Verification("no duration".to_string()))?;
    let frame = source_video.frame_rate.map_or(0.05, |rate| 1.0 / rate);
    if (found - duration).abs() > frame {
        return Err(AcquireError::Verification(format!(
            "{found} s instead of {duration} s"
        )));
    }
    let same = |a: &Option<String>, b: &Option<String>| a.is_some() && a == b;
    if !same(&output.video_codec, &source_video.video_codec) {
        return Err(AcquireError::Verification("the video changed".to_string()));
    }
    if !same(&output.audio_codec, &source_audio.audio_codec) {
        return Err(AcquireError::Verification("the audio changed".to_string()));
    }
    Ok(found)
}

/// Writes `info.json` from what is stored, through the allowlist. Nothing here came from a
/// delivery response, so no fragment URL can reach it.
fn info_json(row: &StreamRow, tags: &Tags) -> Result<String, AcquireError> {
    let stream = &row.stream;
    let info: Value = json!({
        "id": stream.video_id,
        "title": stream.title,
        "channel": stream.channel_name,
        "channel_id": stream.channel_id,
        "description": tags.description,
    });
    serde_json::to_string_pretty(&sanitise_info(&info))
        .map_err(|_| AcquireError::Verification("info.json".to_string()))
}

/// The steps of a cut once its fragments are stored.
pub(crate) struct Assembler<'a> {
    pub(crate) library: &'a Library,
    pub(crate) tools: &'a Ffmpeg,
    pub(crate) row: &'a StreamRow,
}

impl Assembler<'_> {
    /// Finds the clip of an earlier attempt, or records this one with its holds.
    fn open_clip(
        &self,
        request: &CutRequest,
        name: &ExportName,
        tags: &Tags,
        video_itag: u32,
    ) -> Result<ClipRow, AcquireError> {
        let row = self.row;
        let sq_last = request
            .start_sq
            .checked_add(expected_fragments(request, row).saturating_add(SPARE_FRAGMENTS))
            .ok_or(AcquireError::InvalidRequest("the start is out of range"))?;
        let mut database = lock(&self.library.database)?;
        if let Some(found) = database.clip_by_name(&name.name)? {
            let is_same =
                found.clip.sq_first == request.start_sq && found.clip.duration == request.duration;
            return if is_same {
                Ok(found)
            } else {
                Err(AcquireError::NameTaken(name.name.clone()))
            };
        }
        let clip = NewClip {
            name: name.name.clone(),
            kind: match request.kind {
                ExportKind::Addition => ClipKind::Addition,
                ExportKind::FirstCycle | ExportKind::Cycle => ClipKind::Cycle,
            },
            stream_id: row.id,
            video_itag,
            audio_itag: row.stream.audio_itag,
            sq_first: request.start_sq,
            sq_last,
            duration: request.duration,
            metadata: json!({
                "title": tags.title,
                "artist": tags.artist,
                "date": tags.date,
                "description": tags.description,
                "named_at": request.named_at,
            })
            .to_string(),
            poster_time: (request.kind == ExportKind::Addition).then(|| poster_seconds(request)),
        };
        let id = database.insert_held_clip(&clip)?;
        database.clip(id)?.ok_or(AcquireError::NoClip(id))
    }

    /// Plans the cut from stored fragments, which must be there.
    fn plan(&self, clip: &ClipRow, request: &CutRequest) -> Result<FragmentRange, AcquireError> {
        let (first, last) = (clip.clip.sq_first, clip.clip.sq_last);
        let id = self.row.id;
        let (video, audio) = {
            let database = lock(&self.library.database)?;
            (
                database.fragments_in_range(id, clip.clip.video_itag, first, last)?,
                database.fragments_in_range(id, clip.clip.audio_itag, first, last)?,
            )
        };
        let (Some(video_first), Some(audio_first)) = (video.first(), audio.first()) else {
            return Err(AcquireError::InvalidRequest(
                "the cut's fragments are missing",
            ));
        };
        let start = video_first.stream_time;
        let video_range = plan_cut(&pieces(&video), start, request.duration)?;
        let audio_range = plan_cut(&pieces(&audio), audio_first.stream_time, request.duration)?;
        check_air(request, start, start + request.duration)?;
        Ok(FragmentRange {
            sq_first: video_range.sq_first,
            sq_last: video_range.sq_last.max(audio_range.sq_last),
        })
    }

    async fn poster(
        &self,
        video: &Path,
        folder: &Path,
        name: &str,
        at: Option<f64>,
    ) -> Result<(), AcquireError> {
        let path = folder.join(format!("{name}.jpg"));
        match (at, self.row.stream.thumbnail_url.as_deref()) {
            (Some(seconds), _) => self.tools.extract_frame(video, seconds, &path).await?,
            (None, Some(url)) if is_thumbnail(url) => {
                self.tools.fetch_thumbnail(url, &path).await?;
            }
            (None, _) => self.tools.extract_frame(video, 0.0, &path).await?,
        }
        fs::copy(&path, folder.join(format!("{name}-fanart.jpg")))?;
        Ok(())
    }

    /// Joins the fragments and writes the finished folder.
    pub(crate) async fn build(
        &self,
        clip: &ClipRow,
        range: FragmentRange,
        tags: Tags,
    ) -> Result<CutReport, AcquireError> {
        let name = clip.clip.name.clone();
        let directory = self.library.work.join(format!("clip-{}", clip.id));
        let folder = directory.join(&name);
        let video = directory.join("video.webm");
        let audio = directory.join("audio.m4a");
        let store = self.library.store.clone();
        let (id, video_itag, audio_itag) =
            (self.row.id, clip.clip.video_itag, clip.clip.audio_itag);
        let (joined_video, joined_audio, target) = (video.clone(), audio.clone(), folder.clone());
        spawn_blocking(move || -> std::io::Result<()> {
            fs::create_dir_all(&target)?;
            store.concat(id, video_itag, range.sq_first, range.sq_last, &joined_video)?;
            store.concat(id, audio_itag, range.sq_first, range.sq_last, &joined_audio)
        })
        .await
        .map_err(|_| AcquireError::Task)??;
        let output = folder.join(format!("{name}.mp4"));
        let duration = clip.clip.duration;
        self.tools
            .assemble(&Assembly {
                video: video.clone(),
                audio: audio.clone(),
                duration: format!("{duration:.3}"),
                tags: tags.clone(),
                output: output.clone(),
            })
            .await?;
        let found = verify(
            &self.tools.probe(&output).await?,
            &self.tools.probe(&video).await?,
            &self.tools.probe(&audio).await?,
            duration,
        )?;
        self.poster(&video, &folder, &name, clip.clip.poster_time)
            .await?;
        fs::write(
            folder.join(format!("{name}.info.json")),
            info_json(self.row, &tags)?,
        )?;
        fs::remove_file(&video)?;
        fs::remove_file(&audio)?;
        Ok(CutReport {
            clip_id: clip.id,
            name,
            folder,
            range,
            duration: found,
        })
    }
}

/// Renders the name and proposes tags.
fn prepare(
    request: &CutRequest,
    program: &ProgramRow,
    row: &StreamRow,
) -> Result<(ExportName, Tags), AcquireError> {
    if !(request.duration > 0.0 && request.duration <= MAX_CUT_SECONDS) {
        return Err(AcquireError::InvalidRequest(
            "a cut's length is out of range",
        ));
    }
    let name = render_name(&NameParts {
        template: &program.program.name_template,
        name: &program.program.short_name,
        suffix: &program.program.addition_suffix,
        kind: request.kind,
        at: request.named_at,
    })?;
    let tags = proposed_tags(&name, request, program, row);
    Ok((name, tags))
}

/// Holds a cut's fragments, has `ensure` bring them in, and writes its finished folder.
pub(crate) async fn cut_with<F>(
    library: &Library,
    tools: &Ffmpeg,
    request: &CutRequest,
    ensure: F,
) -> Result<CutReport, AcquireError>
where
    F: AsyncFn(&StreamRow, u32, u64, u64) -> Result<(), AcquireError>,
{
    let row = library.registered(&request.stream)?;
    let video_itag = row
        .stream
        .video_itag
        .ok_or_else(|| AcquireError::NoVideo(row.stream.video_id.clone()))?;
    let program = lock(&library.database)?
        .program_of_stream(row.id)?
        .ok_or_else(|| AcquireError::NoProgram(row.stream.video_id.clone()))?;
    let (name, tags) = prepare(request, &program, &row)?;
    let assembler = Assembler {
        library,
        tools,
        row: &row,
    };
    let clip = assembler.open_clip(request, &name, &tags, video_itag)?;
    let tracks = [clip.clip.video_itag, clip.clip.audio_itag];
    let needed = request
        .start_sq
        .saturating_add(expected_fragments(request, &row));
    for itag in tracks {
        ensure(&row, itag, request.start_sq, needed).await?;
    }
    let range = match assembler.plan(&clip, request) {
        Err(AcquireError::Cut(CutError::ShortOfFragments { .. })) => {
            for itag in tracks {
                ensure(&row, itag, needed + 1, needed + SPARE_FRAGMENTS).await?;
            }
            assembler.plan(&clip, request)?
        }
        planned => planned?,
    };
    lock(&library.database)?.set_clip_last_sq(clip.id, range.sq_last)?;
    let report = assembler.build(&clip, range, tags).await?;
    lock(&library.database)?.set_clip_output(clip.id, &report.folder.to_string_lossy())?;
    Ok(report)
}

impl Acquisition {
    /// Holds and fetches a cut's fragments, then writes its finished folder in the work area.
    pub async fn cut(
        &self,
        library: &Library,
        tools: &Ffmpeg,
        request: &CutRequest,
    ) -> Result<CutReport, AcquireError> {
        cut_with(
            library,
            tools,
            request,
            async |row: &StreamRow, itag, first, last| {
                self.ensure_range(library, row, itag, first, last).await
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dalsegno_store::programs::NewProgram;
    use dalsegno_store::streams::NewStream;

    fn request(kind: ExportKind) -> CutRequest {
        CutRequest {
            stream: "AAAAAAAAAAA".to_string(),
            kind,
            start_sq: 100,
            duration: 3_414.0,
            named_at: 1_793_372_160,
            air_time: None,
            override_air_time: false,
            tags: TagEdits::default(),
            poster_seconds: None,
        }
    }

    fn program() -> ProgramRow {
        ProgramRow {
            id: 1,
            program: NewProgram {
                short_name: "Example TV".to_string(),
                channel_id: None,
                name_template: "{name} {datetime}{suffix} ({year})".to_string(),
                addition_suffix: "追加分".to_string(),
                artist: None,
                content_date: Some("2026-01-01".to_string()),
                description: Some("One pass".to_string()),
                output_dir: "Example TV".to_string(),
            },
        }
    }

    fn stream() -> StreamRow {
        StreamRow {
            id: 1,
            stream: NewStream {
                video_id: "AAAAAAAAAAA".to_string(),
                title: "Example".to_string(),
                channel_id: Some("UCexample".to_string()),
                channel_name: Some("The Channel".to_string()),
                thumbnail_url: None,
                video_itag: Some(248),
                audio_itag: 140,
                fragment_len: 5.0,
                registered_at: 0,
            },
        }
    }

    fn facts(duration: Option<f64>, video: &str, audio: &str) -> MediaFacts {
        MediaFacts {
            duration,
            video_codec: Some(video.to_string()),
            audio_codec: Some(audio.to_string()),
            frame_rate: Some(30.0),
        }
    }

    #[test]
    fn an_addition_says_it_is_only_a_part() {
        let (name, tags) = prepare(&request(ExportKind::Addition), &program(), &stream()).unwrap();

        assert_eq!(name.title, tags.title);
        assert_eq!(
            tags.description,
            "Only the part added to the loop, 56:54 long."
        );
    }

    #[test]
    fn a_cycle_takes_the_programs_description() {
        let (_, tags) = prepare(&request(ExportKind::Cycle), &program(), &stream()).unwrap();

        assert_eq!(tags.description, "One pass");
    }

    #[test]
    fn the_artist_falls_back_to_the_channel() {
        let (_, tags) = prepare(&request(ExportKind::Cycle), &program(), &stream()).unwrap();

        assert_eq!(tags.artist, "The Channel");
        assert_eq!(tags.date, "2026-01-01");
    }

    #[test]
    fn edits_win_over_proposals() {
        let edited = CutRequest {
            tags: TagEdits {
                artist: Some("Someone".to_string()),
                date: Some("2026-02-02".to_string()),
                description: Some("Mine".to_string()),
            },
            ..request(ExportKind::Cycle)
        };

        let (_, tags) = prepare(&edited, &program(), &stream()).unwrap();

        assert_eq!(tags.artist, "Someone");
        assert_eq!(tags.date, "2026-02-02");
        assert_eq!(tags.description, "Mine");
    }

    #[test]
    fn a_cut_with_no_length_is_refused() {
        for duration in [0.0, -1.0, f64::NAN, f64::INFINITY, 1e12] {
            let bad = CutRequest {
                duration,
                ..request(ExportKind::Addition)
            };

            assert!(prepare(&bad, &program(), &stream()).is_err());
        }
    }

    #[test]
    fn only_thumbnail_hosts_are_read() {
        assert!(is_thumbnail("https://i.ytimg.com/vi/x/maxresdefault.jpg"));
        assert!(is_thumbnail("https://i9.ytimg.com/vi/x/a.webp"));
        assert!(!is_thumbnail("http://i.ytimg.com/vi/x/a.jpg"));
        assert!(!is_thumbnail("https://ytimg.com.example.invalid/a.jpg"));
        assert!(!is_thumbnail("https://192.168.0.1/a.jpg"));
        assert!(!is_thumbnail("https://i.ytimg.com@192.168.0.1/a.jpg"));
    }

    #[test]
    fn a_cycle_outside_its_air_time_is_blocked_until_overridden() {
        let cycle = CutRequest {
            air_time: Some(AirTime {
                from: 100.0,
                until: Some(200.0),
            }),
            ..request(ExportKind::Cycle)
        };

        assert!(check_air(&cycle, 50.0, 150.0).is_err());
        assert!(check_air(&cycle, 110.0, 150.0).is_ok());
        let forced = CutRequest {
            override_air_time: true,
            ..cycle
        };
        assert!(check_air(&forced, 50.0, 150.0).is_ok());
    }

    #[test]
    fn a_cycle_with_no_air_time_needs_an_override() {
        assert!(check_air(&request(ExportKind::Cycle), 0.0, 1.0).is_err());
    }

    #[test]
    fn an_addition_has_no_air_time_to_check() {
        assert!(check_air(&request(ExportKind::Addition), 0.0, 1.0).is_ok());
    }

    #[test]
    fn a_poster_stays_inside_a_short_addition() {
        let short = CutRequest {
            duration: 100.0,
            ..request(ExportKind::Addition)
        };

        assert_eq!(poster_seconds(&request(ExportKind::Addition)), 90.0);
        assert_eq!(poster_seconds(&short), 50.0);
    }

    #[test]
    fn a_length_within_a_frame_passes() {
        let video = facts(None, "vp9", "");
        let audio = facts(None, "", "aac");

        let found = verify(&facts(Some(10.02), "vp9", "aac"), &video, &audio, 10.0);

        assert_eq!(found.unwrap(), 10.02);
    }

    #[test]
    fn a_length_off_by_more_than_a_frame_fails() {
        let video = facts(None, "vp9", "");
        let audio = facts(None, "", "aac");

        assert!(verify(&facts(Some(10.5), "vp9", "aac"), &video, &audio, 10.0).is_err());
    }

    #[test]
    fn a_changed_codec_fails() {
        let video = facts(None, "vp9", "");
        let audio = facts(None, "", "aac");

        assert!(verify(&facts(Some(10.0), "h264", "aac"), &video, &audio, 10.0).is_err());
        assert!(verify(&facts(Some(10.0), "vp9", "opus"), &video, &audio, 10.0).is_err());
    }

    #[test]
    fn a_missing_stream_fails() {
        let video = facts(None, "vp9", "");
        let audio = facts(None, "", "aac");
        let silent = MediaFacts {
            audio_codec: None,
            ..facts(Some(10.0), "vp9", "")
        };

        assert!(verify(&silent, &video, &audio, 10.0).is_err());
    }

    #[test]
    fn info_json_holds_no_delivery_details() {
        let tags = Tags {
            title: String::new(),
            artist: String::new(),
            date: String::new(),
            description: "Mine".to_string(),
        };

        let text = info_json(&stream(), &tags).unwrap();

        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["id"], "AAAAAAAAAAA");
        assert_eq!(value["description"], "Mine");
        assert!(value.get("formats").is_none());
        assert!(value.get("thumbnail").is_none());
    }
}

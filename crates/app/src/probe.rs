//! Probing a stream: metadata, format choice and fragment checks (FR-S2, FR-S3, D-15).

use crate::error::AcquireError;
use crate::family::{SharedFamily, SwitchReason, Transition};
use crate::session::Clock;
use dalsegno_core::container::{parse_mp4_fragment, read_timing};
pub use dalsegno_core::format::FormatInfo;

use dalsegno_core::format::{choose_audio, video_candidates};
use dalsegno_core::redact::Redacted;
use dalsegno_core::stream::{IpFamily, VideoId, propose_short_name};
use dalsegno_io::http::FragmentClient;
use dalsegno_io::tools::{remux_video_to_mp4, video_keyframes};
use dalsegno_io::ytdlp::{ResolveError, Resolver, StreamFormat, StreamInfo};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

/// `sq` used only to read the head from the response headers.
const HEAD_QUERY_SQ: u64 = 1;
/// Keeps the checked pair behind the head, so neither request is held open.
const PROBE_DISTANCE_FROM_HEAD: u64 = 2;
const MAX_VIDEO_CANDIDATES: usize = 4;
const AAC_LC_OBJECT_TYPE: u8 = 2;

/// Paths of the tools used on probed fragments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeTools {
    /// ffprobe executable.
    pub ffprobe: PathBuf,
    /// ffmpeg executable.
    pub ffmpeg: PathBuf,
}

/// A format that failed a check, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rejection {
    /// Format number.
    pub itag: u32,
    /// What failed.
    pub reason: String,
}

/// What was learned about a stream. Holds no URL.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbeReport {
    /// Stream title.
    pub title: String,
    /// Proposed short name, for a person to confirm.
    pub short_name: String,
    /// Channel name.
    pub channel: Option<String>,
    /// Channel ID.
    pub channel_id: Option<String>,
    /// Thumbnail reference.
    pub thumbnail: Option<String>,
    /// Family the probe used.
    pub family: IpFamily,
    /// Latest `sq` at probe time.
    pub head_sq: u64,
    /// Measured length of one audio fragment, in seconds.
    pub fragment_seconds: Option<f64>,
    /// Chosen audio format.
    pub audio: Option<FormatInfo>,
    /// Chosen video format.
    pub video: Option<FormatInfo>,
    /// Formats that failed a check.
    pub rejected: Vec<Rejection>,
}

/// Reads a stream's metadata, then checks formats until one audio and one video pass.
pub async fn probe_stream<R: Resolver>(
    resolver: &R,
    client: &FragmentClient,
    tools: &ProbeTools,
    video: &VideoId,
    family: &SharedFamily,
    clock: &Clock,
) -> Result<ProbeReport, AcquireError> {
    let (info, used) = read_stream_info(resolver, video, family, clock).await?;
    let audio_choice = choose_audio_format(&info);
    let head_url = audio_choice
        .or(info.formats.first())
        .map(|format| &format.url)
        .ok_or(AcquireError::InvalidRequest(
            "the stream offers no live formats",
        ))?;
    let head_sq = read_head(client, head_url, used).await?;
    let mut rejected = Vec::new();
    let mut fragment_seconds = None;
    let mut audio = None;
    if let Some(format) = audio_choice {
        match probe_audio(client, format, used, head_sq).await {
            Ok(seconds) => {
                fragment_seconds = Some(seconds);
                audio = Some(format.info.clone());
            }
            Err(reason) => rejected.push(rejection(format, reason)),
        }
    }
    let mut chosen_video = None;
    for format in ordered_video(&info).into_iter().take(MAX_VIDEO_CANDIDATES) {
        match probe_video(client, tools, format, used, head_sq).await {
            Ok(()) => {
                chosen_video = Some(format.info.clone());
                break;
            }
            Err(reason) => rejected.push(rejection(format, reason)),
        }
    }
    Ok(ProbeReport {
        short_name: propose_short_name(&info.title),
        title: info.title,
        channel: info.channel,
        channel_id: info.channel_id,
        thumbnail: info.thumbnail,
        family: used,
        head_sq,
        fragment_seconds,
        audio,
        video: chosen_video,
        rejected,
    })
}

fn rejection(format: &StreamFormat, reason: String) -> Rejection {
    Rejection {
        itag: format.info.itag,
        reason,
    }
}

async fn read_stream_info<R: Resolver>(
    resolver: &R,
    video: &VideoId,
    family: &SharedFamily,
    clock: &Clock,
) -> Result<(StreamInfo, IpFamily), AcquireError> {
    loop {
        let used = family.lock().await.active();
        match resolver.stream_info(video, used).await {
            Ok(info) => return Ok((info, used)),
            Err(ResolveError::BotCheck) => {
                let transition =
                    family
                        .lock()
                        .await
                        .on_origin_failure(used, clock(), SwitchReason::BotCheck);
                if transition == Transition::Exhausted {
                    return Err(AcquireError::FamilyExhausted(used));
                }
            }
            Err(ResolveError::Unavailable(message)) => {
                return Err(AcquireError::Unavailable(message));
            }
            Err(ResolveError::Failed(message)) => return Err(AcquireError::Resolve(message)),
        }
    }
}

fn choose_audio_format(info: &StreamInfo) -> Option<&StreamFormat> {
    let infos = format_infos(info);
    let chosen = choose_audio(&infos)?;
    info.formats
        .iter()
        .find(|format| format.info.itag == chosen.itag)
}

fn ordered_video(info: &StreamInfo) -> Vec<&StreamFormat> {
    let infos = format_infos(info);
    video_candidates(&infos)
        .into_iter()
        .filter_map(|candidate| {
            info.formats
                .iter()
                .find(|format| format.info.itag == candidate.itag)
        })
        .collect()
}

fn format_infos(info: &StreamInfo) -> Vec<FormatInfo> {
    info.formats
        .iter()
        .map(|format| format.info.clone())
        .collect()
}

/// Returns the head reported by the origin.
pub async fn read_head(
    client: &FragmentClient,
    url: &Redacted<String>,
    family: IpFamily,
) -> Result<u64, AcquireError> {
    client
        .get(url, HEAD_QUERY_SQ, family)
        .await?
        .head_seqnum
        .ok_or(AcquireError::NoHead)
}

async fn fetch_pair(
    client: &FragmentClient,
    format: &StreamFormat,
    family: IpFamily,
    head_sq: u64,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let first = head_sq
        .checked_sub(PROBE_DISTANCE_FROM_HEAD)
        .ok_or("the stream is too short to probe")?;
    let fetch = async |sq| {
        let response = client
            .get(&format.url, sq, family)
            .await
            .map_err(|error| error.to_string())?;
        if response.status == 200 && !response.body.is_empty() {
            Ok(response.body)
        } else {
            Err(format!(
                "status {} with {} bytes",
                response.status,
                response.body.len()
            ))
        }
    };
    Ok((fetch(first).await?, fetch(first + 1).await?))
}

async fn probe_audio(
    client: &FragmentClient,
    format: &StreamFormat,
    family: IpFamily,
    head_sq: u64,
) -> Result<f64, String> {
    let (first, second) = fetch_pair(client, format, family, head_sq).await?;
    check_audio_pair(&first, &second)
}

async fn probe_video(
    client: &FragmentClient,
    tools: &ProbeTools,
    format: &StreamFormat,
    family: IpFamily,
    head_sq: u64,
) -> Result<(), String> {
    let (first, second) = fetch_pair(client, format, family, head_sq).await?;
    check_video_pair(tools, &first, &second).await
}

/// Checks that two adjacent fragments are contiguous AAC-LC, and returns the first's duration.
fn check_audio_pair(first: &[u8], second: &[u8]) -> Result<f64, String> {
    let a = parse_mp4_fragment(first).map_err(|error| error.to_string())?;
    let b = parse_mp4_fragment(second).map_err(|error| error.to_string())?;
    if a.audio_config.first().map(|byte| byte >> 3) != Some(AAC_LC_OBJECT_TYPE) {
        return Err("audio is not AAC-LC".to_string());
    }
    if a.timescale != b.timescale || a.start_ticks + a.duration_ticks != b.start_ticks {
        return Err("audio fragments are not contiguous".to_string());
    }
    Ok(a.duration())
}

/// Checks keyframe-first, concatenation and mp4 compatibility on two adjacent fragments.
async fn check_video_pair(tools: &ProbeTools, first: &[u8], second: &[u8]) -> Result<(), String> {
    read_timing(first).map_err(|error| format!("timing unreadable: {error}"))?;
    let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
    let path = |name: &str| directory.path().join(name);
    let write =
        |target: &Path, bytes: &[u8]| fs::write(target, bytes).map_err(|error| error.to_string());
    write(&path("first"), first)?;
    write(&path("second"), second)?;
    write(&path("joined"), &[first, second].concat())?;
    let keyframes = async |target: PathBuf| {
        video_keyframes(&tools.ffprobe, &target)
            .await
            .map_err(|error| error.to_string())
    };
    let first_packets = keyframes(path("first")).await?;
    let second_packets = keyframes(path("second")).await?;
    if first_packets.first() != Some(&true) || second_packets.first() != Some(&true) {
        return Err("a fragment does not start with a keyframe".to_string());
    }
    let joined_packets = keyframes(path("joined")).await?;
    if joined_packets.len() != first_packets.len() + second_packets.len() {
        return Err("concatenation loses packets".to_string());
    }
    remux_video_to_mp4(&tools.ffmpeg, &path("joined"), &path("joined.mp4"))
        .await
        .map_err(|error| format!("mp4 compatibility: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dalsegno_test_support::require_fixtures;

    fn numbered_files(directory: &Path) -> Vec<PathBuf> {
        let mut files: Vec<(u64, PathBuf)> = fs::read_dir(directory)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                let sq = path.file_name()?.to_str()?.parse().ok()?;
                Some((sq, path))
            })
            .collect();
        files.sort();
        files.into_iter().map(|(_, path)| path).collect()
    }

    fn adjacent_pair(directory: &Path) -> (Vec<u8>, Vec<u8>) {
        let files = numbered_files(directory);
        (fs::read(&files[0]).unwrap(), fs::read(&files[1]).unwrap())
    }

    fn system_tools() -> Option<ProbeTools> {
        Some(ProbeTools {
            ffprobe: dalsegno_io::tools::find_program("ffprobe")?,
            ffmpeg: dalsegno_io::tools::find_program("ffmpeg")?,
        })
    }

    #[test]
    fn garbage_audio_is_rejected() {
        assert!(check_audio_pair(b"garbage", b"garbage").is_err());
    }

    #[test]
    fn real_audio_pair_passes_and_reports_its_length() {
        let fixtures = require_fixtures!();
        let (first, second) = adjacent_pair(&fixtures.join("frag-140"));

        let seconds = check_audio_pair(&first, &second).unwrap();

        assert!((4.9..5.1).contains(&seconds), "{seconds}");
    }

    #[test]
    fn out_of_order_audio_is_rejected() {
        let fixtures = require_fixtures!();
        let (first, second) = adjacent_pair(&fixtures.join("frag-140"));

        let reason = check_audio_pair(&second, &first).unwrap_err();

        assert!(reason.contains("contiguous"), "{reason}");
    }

    #[tokio::test]
    async fn real_video_pair_passes() {
        let fixtures = require_fixtures!();
        let Some(tools) = system_tools() else { return };
        let (first, second) = adjacent_pair(&fixtures.join("frag-248"));

        check_video_pair(&tools, &first, &second).await.unwrap();
    }

    #[tokio::test]
    async fn fragments_cut_after_the_first_keyframe_are_rejected() {
        let fixtures = require_fixtures!();
        let Some(tools) = system_tools() else { return };
        let (first, second) = adjacent_pair(&fixtures.join("frag-248"));

        let result = check_video_pair(&tools, &first[..first.len() / 2], &second[4096..]).await;

        assert!(result.is_err());
    }
}

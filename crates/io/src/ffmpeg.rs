//! ffmpeg and ffprobe for export: detection, assembly and frames (`docs/design/export.md`).
//!
//! Every call takes an argument vector, so nothing passes through a shell.

use crate::tools::ToolError;
use serde::Deserialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const DETECT_TIMEOUT: Duration = Duration::from_secs(1800);
const ASSEMBLE_TIMEOUT: Duration = Duration::from_secs(3600);
const FRAME_TIMEOUT: Duration = Duration::from_secs(600);
const PROBE_TIMEOUT: Duration = Duration::from_secs(120);
/// ffmpeg's complaint about concatenated WebM, which is expected.
const EXPECTED_WARNING: &str = "unknown-length element";
const STDERR_TAIL_LINES: usize = 5;

const BLACK_FILTER: &str = "blackdetect=d=0.03:pix_th=0.15:pic_th=0.90";

/// How hard silence is looked for. Some versions have no complete silence between songs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SilenceLevel {
    /// The measured parameters for use around a candidate.
    Fine,
    /// Loosened, for when `Fine` finds nothing.
    Relaxed,
}

impl SilenceLevel {
    fn filter(self) -> &'static str {
        match self {
            Self::Fine => "silencedetect=noise=-40dB:d=0.1",
            Self::Relaxed => "silencedetect=noise=-35dB:d=0.1",
        }
    }
}

/// The tags of an output file (FR-E8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tags {
    /// `title`.
    pub title: String,
    /// `artist`.
    pub artist: String,
    /// `date`.
    pub date: String,
    /// `description`.
    pub description: String,
}

/// One muxing job. Streams are always copied (G4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assembly {
    /// Concatenated video, starting at the cut.
    pub video: PathBuf,
    /// Concatenated audio, starting at the cut.
    pub audio: PathBuf,
    /// Length of the cut, in seconds.
    pub duration: String,
    /// Tags to write.
    pub tags: Tags,
    /// Output file.
    pub output: PathBuf,
}

/// What ffprobe says about a finished file.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaFacts {
    /// Length of the container, in seconds. Concatenated WebM does not state one.
    pub duration: Option<f64>,
    /// Video codec.
    pub video_codec: Option<String>,
    /// Audio codec.
    pub audio_codec: Option<String>,
    /// Frames per second of the video.
    pub frame_rate: Option<f64>,
}

/// The ffmpeg and ffprobe programs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ffmpeg {
    /// ffmpeg executable.
    pub ffmpeg: PathBuf,
    /// ffprobe executable.
    pub ffprobe: PathBuf,
}

impl Ffmpeg {
    /// Returns the `blackdetect` log of a whole file.
    pub async fn detect_black(&self, video: &Path) -> Result<String, ToolError> {
        let args = detection_args(video, "-vf", BLACK_FILTER, "-an");
        let output = run(&self.ffmpeg, &args, DETECT_TIMEOUT).await?;
        Ok(output.stderr)
    }

    /// Returns the `silencedetect` log of a whole file.
    pub async fn detect_silence(
        &self,
        audio: &Path,
        level: SilenceLevel,
    ) -> Result<String, ToolError> {
        let args = detection_args(audio, "-af", level.filter(), "-vn");
        let output = run(&self.ffmpeg, &args, DETECT_TIMEOUT).await?;
        Ok(output.stderr)
    }

    /// Writes the mp4 of a cut, with the index at the front.
    pub async fn assemble(&self, job: &Assembly) -> Result<(), ToolError> {
        run(&self.ffmpeg, &assembly_args(job), ASSEMBLE_TIMEOUT)
            .await
            .map(|_| ())
    }

    /// Saves one frame as a jpg, as an addition's poster.
    pub async fn extract_frame(
        &self,
        video: &Path,
        seconds: f64,
        output: &Path,
    ) -> Result<(), ToolError> {
        let args = args_of([
            "-v".into(),
            "error".into(),
            "-y".into(),
            "-ss".into(),
            format!("{seconds:.3}").into(),
            "-i".into(),
            video.into(),
            "-frames:v".into(),
            "1".into(),
            "-q:v".into(),
            "2".into(),
            output.into(),
        ]);
        run(&self.ffmpeg, &args, FRAME_TIMEOUT).await.map(|_| ())
    }

    /// Converts a stream's thumbnail to a jpg, over HTTPS only.
    pub async fn fetch_thumbnail(&self, url: &str, output: &Path) -> Result<(), ToolError> {
        let args = args_of([
            "-v".into(),
            "error".into(),
            "-y".into(),
            "-protocol_whitelist".into(),
            "https,tls,tcp".into(),
            "-i".into(),
            url.into(),
            "-frames:v".into(),
            "1".into(),
            "-q:v".into(),
            "2".into(),
            output.into(),
        ]);
        run(&self.ffmpeg, &args, FRAME_TIMEOUT).await.map(|_| ())
    }

    /// Saves a grid of frames one every few seconds, for choosing a boundary by eye.
    pub async fn contact_sheet(&self, video: &Path, output: &Path) -> Result<(), ToolError> {
        let args = args_of([
            "-v".into(),
            "error".into(),
            "-y".into(),
            "-i".into(),
            video.into(),
            "-vf".into(),
            "fps=1,scale=320:-2,tile=6x7".into(),
            "-frames:v".into(),
            "1".into(),
            "-q:v".into(),
            "4".into(),
            output.into(),
        ]);
        run(&self.ffmpeg, &args, FRAME_TIMEOUT).await.map(|_| ())
    }

    /// Reads the facts that export verifies.
    pub async fn probe(&self, file: &Path) -> Result<MediaFacts, ToolError> {
        let args = args_of([
            "-v".into(),
            "error".into(),
            "-show_entries".into(),
            "format=duration:stream=codec_type,codec_name,avg_frame_rate".into(),
            "-of".into(),
            "json".into(),
            file.into(),
        ]);
        let output = run(&self.ffprobe, &args, PROBE_TIMEOUT).await?;
        parse_probe(&output.stdout)
            .ok_or_else(|| ToolError::Failed("ffprobe gave unreadable output".to_string()))
    }
}

fn args_of<const N: usize>(items: [OsString; N]) -> Vec<OsString> {
    items.into_iter().collect()
}

fn detection_args(file: &Path, flag: &str, filter: &str, other_track: &str) -> Vec<OsString> {
    args_of([
        "-hide_banner".into(),
        "-nostats".into(),
        "-i".into(),
        file.into(),
        other_track.into(),
        flag.into(),
        filter.into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ])
}

fn assembly_args(job: &Assembly) -> Vec<OsString> {
    let tag = |key: &str, value: &str| -> [OsString; 2] {
        ["-metadata".into(), format!("{key}={value}").into()]
    };
    let mut args = args_of([
        "-v".into(),
        "warning".into(),
        "-y".into(),
        "-i".into(),
        job.video.as_os_str().to_owned(),
        "-i".into(),
        job.audio.as_os_str().to_owned(),
        "-map".into(),
        "0:v".into(),
        "-map".into(),
        "1:a".into(),
        "-c".into(),
        "copy".into(),
        "-t".into(),
        job.duration.clone().into(),
        "-movflags".into(),
        "+faststart".into(),
    ]);
    args.extend(tag("title", &job.tags.title));
    args.extend(tag("artist", &job.tags.artist));
    args.extend(tag("date", &job.tags.date));
    args.extend(tag("description", &job.tags.description));
    args.push(job.output.as_os_str().to_owned());
    args
}

#[derive(Deserialize)]
struct ProbeOutput {
    format: ProbeFormat,
    #[serde(default)]
    streams: Vec<ProbeStream>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct ProbeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
    avg_frame_rate: Option<String>,
}

fn parse_probe(stdout: &str) -> Option<MediaFacts> {
    let output: ProbeOutput = serde_json::from_str(stdout).ok()?;
    let duration = output.format.duration.and_then(|text| text.parse().ok());
    let of_type = |kind: &str| {
        output
            .streams
            .iter()
            .find(|stream| stream.codec_type.as_deref() == Some(kind))
    };
    let video = of_type("video");
    Some(MediaFacts {
        duration,
        video_codec: video.and_then(|stream| stream.codec_name.clone()),
        audio_codec: of_type("audio").and_then(|stream| stream.codec_name.clone()),
        frame_rate: video
            .and_then(|stream| stream.avg_frame_rate.as_deref())
            .and_then(parse_rate),
    })
}

fn parse_rate(rate: &str) -> Option<f64> {
    let (top, bottom) = rate.split_once('/')?;
    let (top, bottom): (f64, f64) = (top.parse().ok()?, bottom.parse().ok()?);
    (bottom != 0.0).then(|| top / bottom)
}

struct Output {
    stdout: String,
    stderr: String,
}

async fn run(program: &Path, args: &[OsString], limit: Duration) -> Result<Output, ToolError> {
    let name = program
        .file_name()
        .unwrap_or(program.as_os_str())
        .to_string_lossy()
        .into_owned();
    let run = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(limit, run)
        .await
        .map_err(|_| ToolError::NotRun(name.clone(), "timed out".to_string()))?
        .map_err(|error| ToolError::NotRun(name.clone(), error.kind().to_string()))?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.status.success() {
        return Ok(Output {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr,
        });
    }
    Err(ToolError::Failed(format!(
        "{name}: {}",
        stderr_tail(&stderr)
    )))
}

/// Keeps the end of a failure's log, without the warning that every WebM produces.
fn stderr_tail(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| !line.contains(EXPECTED_WARNING))
        .collect();
    let start = lines.len().saturating_sub(STDERR_TAIL_LINES);
    lines[start..].join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::find_program;
    use dalsegno_core::export::{parse_black, parse_silence};
    use tempfile::tempdir;

    fn job() -> Assembly {
        Assembly {
            video: "/work/video.webm".into(),
            audio: "/work/audio.m4a".into(),
            duration: "42244.700".to_string(),
            tags: Tags {
                title: "Example TV 20261030-2356".to_string(),
                artist: "Example".to_string(),
                date: "2026-01-01".to_string(),
                description: "Only a part".to_string(),
            },
            output: "/work/out.mp4".into(),
        }
    }

    fn strings(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn assembly_never_encodes() {
        let args = strings(&assembly_args(&job()));

        let codec = args.iter().position(|arg| arg == "-c").unwrap();
        assert_eq!(args[codec + 1], "copy");
        assert!(!args.iter().any(|arg| arg == "-vf" || arg == "-af"));
    }

    #[test]
    fn assembly_puts_the_index_first() {
        let args = strings(&assembly_args(&job()));

        assert!(args.iter().any(|arg| arg == "+faststart"));
    }

    #[test]
    fn assembly_seeks_nothing() {
        // Fragments start at the cut, so a seek would only add risk (D-06).
        assert!(!strings(&assembly_args(&job())).iter().any(|a| a == "-ss"));
    }

    #[test]
    fn tags_stay_one_argument_each() {
        let hostile = Assembly {
            tags: Tags {
                title: "a -y /etc/passwd; rm -rf /".to_string(),
                ..job().tags
            },
            ..job()
        };

        let args = strings(&assembly_args(&hostile));

        assert!(args.contains(&"title=a -y /etc/passwd; rm -rf /".to_string()));
    }

    #[test]
    fn the_output_comes_last() {
        let args = strings(&assembly_args(&job()));

        assert_eq!(args.last().unwrap(), "/work/out.mp4");
    }

    #[test]
    fn silence_loosens_as_measured() {
        assert!(SilenceLevel::Fine.filter().contains("-40dB"));
        assert!(SilenceLevel::Relaxed.filter().contains("-35dB"));
    }

    #[test]
    fn probe_output_is_read() {
        let json = r#"{"format":{"duration":"12.500000"},"streams":[
            {"codec_type":"video","codec_name":"vp9","avg_frame_rate":"30000/1001"},
            {"codec_type":"audio","codec_name":"aac","avg_frame_rate":"0/0"}]}"#;

        let facts = parse_probe(json).unwrap();

        assert_eq!(facts.duration, Some(12.5));
        assert_eq!(facts.video_codec.as_deref(), Some("vp9"));
        assert_eq!(facts.audio_codec.as_deref(), Some("aac"));
        assert!((facts.frame_rate.unwrap() - 29.97).abs() < 0.01);
    }

    #[test]
    fn probe_output_can_lack_a_duration() {
        assert_eq!(parse_probe(r#"{"format":{}}"#).unwrap().duration, None);
    }

    #[test]
    fn probe_output_that_is_not_json_is_unreadable() {
        assert_eq!(parse_probe("not json"), None);
    }

    #[test]
    fn a_rate_of_zero_over_zero_is_no_rate() {
        assert_eq!(parse_rate("0/0"), None);
        assert_eq!(parse_rate("25"), None);
    }

    #[test]
    fn the_expected_warning_is_filtered_from_failures() {
        let stderr = "one\n[matroska] unknown-length element at 1\ntwo\n";

        assert_eq!(stderr_tail(stderr), "one | two");
    }

    #[test]
    fn a_long_failure_keeps_its_end() {
        let stderr: String = (0..20).map(|index| format!("line {index}\n")).collect();

        assert!(stderr_tail(&stderr).starts_with("line 15"));
    }

    #[tokio::test]
    async fn a_missing_program_is_reported() {
        let tools = Ffmpeg {
            ffmpeg: "/definitely/not/ffmpeg".into(),
            ffprobe: "/definitely/not/ffprobe".into(),
        };

        assert!(matches!(
            tools.detect_black(Path::new("/x")).await,
            Err(ToolError::NotRun(..))
        ));
    }

    fn real_tools() -> Option<Ffmpeg> {
        Some(Ffmpeg {
            ffmpeg: find_program("ffmpeg")?,
            ffprobe: find_program("ffprobe")?,
        })
    }

    async fn synth(ffmpeg: &Path, args: &[&str], output: &Path) {
        let status = Command::new(ffmpeg)
            .args(["-v", "error", "-y"])
            .args(args)
            .arg(output)
            .output()
            .await
            .unwrap();
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    // Runs where ffmpeg is installed. Everything is generated, so it needs no fixtures.
    #[tokio::test]
    async fn a_real_cut_is_detected_assembled_and_verified() {
        let Some(tools) = real_tools() else {
            return;
        };
        let dir = tempdir().unwrap();
        let (video, audio) = (dir.path().join("v.mkv"), dir.path().join("a.m4a"));
        let black_gap = "color=black:s=64x64:r=10:d=2";
        let picture = "testsrc=s=64x64:r=10:d=3";
        synth(
            &tools.ffmpeg,
            &[
                "-f",
                "lavfi",
                "-i",
                picture,
                "-f",
                "lavfi",
                "-i",
                black_gap,
                "-f",
                "lavfi",
                "-i",
                picture,
                "-filter_complex",
                "[0][1][2]concat=n=3:v=1",
                "-c:v",
                "mpeg4",
            ],
            &video,
        )
        .await;
        synth(
            &tools.ffmpeg,
            &[
                "-f",
                "lavfi",
                "-i",
                "sine=f=440:d=3",
                "-f",
                "lavfi",
                "-i",
                "anullsrc=r=44100:cl=mono",
                "-f",
                "lavfi",
                "-i",
                "sine=f=440:d=3",
                "-filter_complex",
                "[1]atrim=0:2[gap];[0][gap][2]concat=n=3:v=0:a=1",
                "-c:a",
                "aac",
            ],
            &audio,
        )
        .await;

        let black = tools.detect_black(&video).await.unwrap();
        let silence = tools
            .detect_silence(&audio, SilenceLevel::Fine)
            .await
            .unwrap();
        let output = dir.path().join("out.mp4");
        tools
            .assemble(&Assembly {
                video: video.clone(),
                audio,
                duration: "6.000".to_string(),
                output: output.clone(),
                ..job()
            })
            .await
            .unwrap();
        let frame = dir.path().join("poster.jpg");
        tools.extract_frame(&video, 1.0, &frame).await.unwrap();
        let sheet = dir.path().join("sheet.jpg");
        tools.contact_sheet(&video, &sheet).await.unwrap();

        let near_three = |start: f64| (start - 3.0).abs() < 0.2;
        assert!(near_three(parse_black(&black)[0].start), "{black}");
        assert!(
            near_three(parse_silence(&silence, 8.0)[0].start),
            "{silence}"
        );
        let facts = tools.probe(&output).await.unwrap();
        assert!((facts.duration.unwrap() - 6.0).abs() < 0.2, "{facts:?}");
        assert_eq!(facts.video_codec.as_deref(), Some("mpeg4"));
        assert_eq!(facts.audio_codec.as_deref(), Some("aac"));
        assert!(frame.exists() && sheet.exists());
    }
}

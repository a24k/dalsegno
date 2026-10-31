//! The only module that knows yt-dlp's arguments and JSON (NFR-7).

use dalsegno_core::format::FormatInfo;
use dalsegno_core::redact::Redacted;
use dalsegno_core::stream::{IpFamily, VideoId};
use serde::Deserialize;
use std::ffi::OsString;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const LIVE_FRAGMENT_PROTOCOL: &str = "http_dash_segments_generator";
const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_MESSAGE_CHARS: usize = 200;
const REDACTED_TOKEN: &str = "[redacted]";
const BOT_CHECK_MARKERS: [&str; 2] = ["not a bot", "confirm you"];
const UNAVAILABLE_MARKERS: [&str; 5] = [
    "private video",
    "video unavailable",
    "has been removed",
    "has ended",
    "is offline",
];

/// Why a URL could not be resolved. Messages are sanitised and safe to log.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResolveError {
    /// The origin asked for a bot check, which concerns the whole IP family (D-26).
    #[error("the origin asked for a bot check")]
    BotCheck,
    /// This stream is gone, which concerns only this stream (D-26).
    #[error("stream unavailable: {0}")]
    Unavailable(String),
    /// Anything else, including a yt-dlp that needs updating.
    #[error("yt-dlp failed: {0}")]
    Failed(String),
}

/// A format together with its URL.
#[derive(Debug, Clone)]
pub struct StreamFormat {
    /// What the format is.
    pub info: FormatInfo,
    /// Where its fragments are fetched from.
    pub url: Redacted<String>,
}

/// What a stream reports about itself.
#[derive(Debug, Clone)]
pub struct StreamInfo {
    /// Stream title.
    pub title: String,
    /// Channel name.
    pub channel: Option<String>,
    /// Channel ID.
    pub channel_id: Option<String>,
    /// Thumbnail reference.
    pub thumbnail: Option<String>,
    /// Formats delivered as live fragments.
    pub formats: Vec<StreamFormat>,
}

/// Turns a stream into fragment URLs. Implemented by a fake in tests.
pub trait Resolver: Send + Sync + 'static {
    /// Resolves the URL of one format over one IP family.
    fn resolve(
        &self,
        video: &VideoId,
        itag: u32,
        family: IpFamily,
    ) -> impl Future<Output = Result<Redacted<String>, ResolveError>> + Send;

    /// Reads the stream's metadata and all its formats.
    fn stream_info(
        &self,
        video: &VideoId,
        family: IpFamily,
    ) -> impl Future<Output = Result<StreamInfo, ResolveError>> + Send;
}

/// Runs the yt-dlp executable. Arguments are vectors, never a shell line.
#[derive(Debug, Clone)]
pub struct YtDlp {
    program: PathBuf,
    cookies: Option<PathBuf>,
}

impl YtDlp {
    /// Uses the given executable, and a cookie file if one is set.
    pub fn new(program: impl Into<PathBuf>, cookies: Option<PathBuf>) -> Self {
        Self {
            program: program.into(),
            cookies,
        }
    }

    fn base_args(&self, family: IpFamily) -> Vec<OsString> {
        let flag = match family {
            IpFamily::V4 => "-4",
            IpFamily::V6 => "-6",
        };
        let cookies = self
            .cookies
            .iter()
            .flat_map(|path| [OsString::from("--cookies"), path.into()]);
        [flag, "--live-from-start"]
            .map(OsString::from)
            .into_iter()
            .chain(cookies)
            .collect()
    }

    fn resolve_args(&self, video: &VideoId, itag: u32, family: IpFamily) -> Vec<OsString> {
        let selection = ["-f".to_string(), itag.to_string()];
        let target = ["-j".to_string(), "--".to_string(), video.watch_url()];
        self.base_args(family)
            .into_iter()
            .chain(selection.into_iter().chain(target).map(OsString::from))
            .collect()
    }

    fn info_args(&self, video: &VideoId, family: IpFamily) -> Vec<OsString> {
        let target = ["-j".to_string(), "--".to_string(), video.watch_url()];
        self.base_args(family)
            .into_iter()
            .chain(target.map(OsString::from))
            .collect()
    }

    async fn run(&self, args: Vec<OsString>) -> Result<Vec<u8>, ResolveError> {
        let run = Command::new(&self.program)
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output();
        let output = tokio::time::timeout(TIMEOUT, run)
            .await
            .map_err(|_| ResolveError::Failed("timed out".to_string()))?
            .map_err(|error| ResolveError::Failed(format!("not started: {}", error.kind())))?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(classify_failure(&String::from_utf8_lossy(&output.stderr)))
        }
    }
}

impl Resolver for YtDlp {
    async fn resolve(
        &self,
        video: &VideoId,
        itag: u32,
        family: IpFamily,
    ) -> Result<Redacted<String>, ResolveError> {
        let stdout = self.run(self.resolve_args(video, itag, family)).await?;
        parse_url(&stdout)
    }

    async fn stream_info(
        &self,
        video: &VideoId,
        family: IpFamily,
    ) -> Result<StreamInfo, ResolveError> {
        let stdout = self.run(self.info_args(video, family)).await?;
        parse_stream_info(&stdout)
    }
}

#[derive(Debug, Deserialize)]
struct RawInfo {
    url: Option<String>,
    title: Option<String>,
    channel: Option<String>,
    channel_id: Option<String>,
    thumbnail: Option<String>,
    #[serde(default)]
    formats: Vec<RawFormat>,
}

#[derive(Debug, Deserialize)]
struct RawFormat {
    format_id: Option<String>,
    url: Option<String>,
    protocol: Option<String>,
    ext: Option<String>,
    vcodec: Option<String>,
    acodec: Option<String>,
    height: Option<f64>,
    fps: Option<f64>,
    tbr: Option<f64>,
    has_drm: Option<serde_json::Value>,
}

impl RawFormat {
    fn into_stream_format(self) -> Option<StreamFormat> {
        if self.protocol.as_deref() != Some(LIVE_FRAGMENT_PROTOCOL) {
            return None;
        }
        let info = FormatInfo {
            itag: self.format_id?.parse().ok()?,
            ext: self.ext.unwrap_or_default(),
            vcodec: self.vcodec.filter(|codec| codec != "none"),
            acodec: self.acodec.filter(|codec| codec != "none"),
            height: self.height.map(|height| height as u32),
            fps: self.fps,
            tbr: self.tbr,
            has_drm: !matches!(
                self.has_drm,
                None | Some(serde_json::Value::Null | serde_json::Value::Bool(false))
            ),
        };
        Some(StreamFormat {
            info,
            url: Redacted::new(self.url?),
        })
    }
}

// serde_json quotes offending values in its messages, and these values can be URLs.
fn parse_raw(stdout: &[u8]) -> Result<RawInfo, ResolveError> {
    serde_json::from_slice(stdout)
        .map_err(|_| ResolveError::Failed("unreadable yt-dlp output".to_string()))
}

fn parse_url(stdout: &[u8]) -> Result<Redacted<String>, ResolveError> {
    parse_raw(stdout)?
        .url
        .map(Redacted::new)
        .ok_or_else(|| ResolveError::Failed("no URL in yt-dlp output".to_string()))
}

fn parse_stream_info(stdout: &[u8]) -> Result<StreamInfo, ResolveError> {
    let raw = parse_raw(stdout)?;
    Ok(StreamInfo {
        title: raw.title.unwrap_or_default(),
        channel: raw.channel,
        channel_id: raw.channel_id,
        thumbnail: raw.thumbnail,
        formats: raw
            .formats
            .into_iter()
            .filter_map(RawFormat::into_stream_format)
            .collect(),
    })
}

fn classify_failure(stderr: &str) -> ResolveError {
    let lower = stderr.to_lowercase();
    let contains_any = |markers: &[&str]| markers.iter().any(|marker| lower.contains(marker));
    if contains_any(&BOT_CHECK_MARKERS) {
        return ResolveError::BotCheck;
    }
    let message = sanitise(stderr.lines().last().unwrap_or_default());
    if contains_any(&UNAVAILABLE_MARKERS) {
        ResolveError::Unavailable(message)
    } else {
        ResolveError::Failed(message)
    }
}

fn sanitise(line: &str) -> String {
    let words: Vec<&str> = line
        .split_whitespace()
        .map(|word| {
            if is_sensitive(word) {
                REDACTED_TOKEN
            } else {
                word
            }
        })
        .collect();
    words.join(" ").chars().take(MAX_MESSAGE_CHARS).collect()
}

fn is_sensitive(word: &str) -> bool {
    let bracketed = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && !"[]".contains(c));
    let with_colons = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != ':');
    word.contains("://")
        || word.contains('=')
        || bracketed.parse::<SocketAddr>().is_ok()
        || bracketed.trim_matches(['[', ']']).parse::<IpAddr>().is_ok()
        || with_colons.parse::<IpAddr>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: &str = r#"{
        "url": "https://host/videoplayback?itag=140&ip=192.0.2.1",
        "title": "Example TV -An Archive-",
        "channel": "Example",
        "channel_id": "UCexample",
        "thumbnail": "https://host/thumb.jpg",
        "formats": [
            {"format_id": "sb0", "protocol": "mhtml", "url": "https://host/sb"},
            {"format_id": "140", "protocol": "http_dash_segments_generator",
             "url": "https://host/a?ip=192.0.2.1", "ext": "m4a", "vcodec": "none",
             "acodec": "mp4a.40.2", "height": null, "fps": null, "tbr": 144.0,
             "has_drm": false},
            {"format_id": "248", "protocol": "http_dash_segments_generator",
             "url": "https://host/v?ip=192.0.2.1", "ext": "webm", "vcodec": "vp9",
             "acodec": "none", "height": 1080, "fps": 30, "tbr": 1800.5},
            {"format_id": "999", "protocol": "http_dash_segments_generator",
             "url": "https://host/d", "has_drm": "maybe"},
            {"format_id": "140-drc", "protocol": "http_dash_segments_generator",
             "url": "https://host/x"}
        ]
    }"#;

    fn video() -> VideoId {
        VideoId::parse("abcDEF123_-").unwrap()
    }

    fn text(args: &[OsString]) -> Vec<String> {
        args.iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn resolve_args_carry_family_itag_and_end_of_options() {
        let args = YtDlp::new("yt-dlp", None).resolve_args(&video(), 140, IpFamily::V6);

        assert_eq!(
            text(&args),
            [
                "-6",
                "--live-from-start",
                "-f",
                "140",
                "-j",
                "--",
                "https://www.youtube.com/watch?v=abcDEF123_-"
            ]
        );
    }

    #[test]
    fn cookies_are_passed_by_path() {
        let tool = YtDlp::new("yt-dlp", Some(PathBuf::from("/config/cookies.txt")));

        let args = text(&tool.info_args(&video(), IpFamily::V4));

        assert_eq!(
            args[..4],
            [
                "-4",
                "--live-from-start",
                "--cookies",
                "/config/cookies.txt"
            ]
        );
        assert!(!args.contains(&"-f".to_string()));
    }

    #[test]
    fn reads_the_resolved_url() {
        let url = parse_url(INFO.as_bytes()).unwrap();

        assert!(url.expose().contains("itag=140"));
    }

    #[test]
    fn missing_url_is_an_error() {
        assert_eq!(
            parse_url(b"{}").unwrap_err(),
            ResolveError::Failed("no URL in yt-dlp output".to_string())
        );
    }

    #[test]
    fn keeps_only_live_fragment_formats_with_numeric_itags() {
        let info = parse_stream_info(INFO.as_bytes()).unwrap();

        let itags: Vec<u32> = info.formats.iter().map(|format| format.info.itag).collect();
        assert_eq!(itags, [140, 248, 999]);
    }

    #[test]
    fn maps_format_fields() {
        let info = parse_stream_info(INFO.as_bytes()).unwrap();

        let audio = &info.formats[0].info;
        assert_eq!(audio.vcodec, None);
        assert_eq!(audio.acodec.as_deref(), Some("mp4a.40.2"));
        let video = &info.formats[1].info;
        assert_eq!(video.acodec, None);
        assert_eq!(video.height, Some(1080));
        assert_eq!(video.fps, Some(30.0));
        assert!(!video.has_drm);
        assert!(info.formats[2].info.has_drm);
    }

    #[test]
    fn maps_stream_metadata() {
        let info = parse_stream_info(INFO.as_bytes()).unwrap();

        assert_eq!(info.title, "Example TV -An Archive-");
        assert_eq!(info.channel.as_deref(), Some("Example"));
        assert_eq!(info.channel_id.as_deref(), Some("UCexample"));
    }

    #[test]
    fn debug_output_hides_format_urls() {
        let info = parse_stream_info(INFO.as_bytes()).unwrap();

        assert!(!format!("{info:?}").contains("192.0.2.1"));
    }

    #[test]
    fn unreadable_output_does_not_quote_the_input() {
        let error = parse_stream_info(br#"{"url": 1, "x": "https://host/?ip=192.0.2.1"}"#)
            .unwrap_err()
            .to_string();

        assert!(!error.contains("192.0.2.1"), "{error}");
    }

    #[test]
    fn bot_check_is_recognised() {
        let stderr = "ERROR: [youtube] x: Sign in to confirm you're not a bot.";

        assert_eq!(classify_failure(stderr), ResolveError::BotCheck);
    }

    #[test]
    fn unavailable_stream_is_recognised() {
        assert!(matches!(
            classify_failure("ERROR: [youtube] x: Private video. Sign in"),
            ResolveError::Unavailable(_)
        ));
        assert!(matches!(
            classify_failure("ERROR: This live event has ended."),
            ResolveError::Unavailable(_)
        ));
    }

    #[test]
    fn other_failures_keep_the_last_line() {
        assert_eq!(
            classify_failure("WARNING: first\nERROR: something broke"),
            ResolveError::Failed("ERROR: something broke".to_string())
        );
    }

    #[test]
    fn failure_messages_drop_urls_and_addresses() {
        let error = classify_failure(
            "ERROR: HTTP 500 https://host/?ip=192.0.2.1 from 192.0.2.1, ip=2001:db8::1 [2001:db8::2]",
        )
        .to_string();

        assert!(!error.contains("192.0.2.1"), "{error}");
        assert!(!error.contains("2001:db8"), "{error}");
        assert!(error.contains("HTTP 500"), "{error}");
    }

    #[test]
    fn failure_messages_drop_addresses_with_ports() {
        let error = classify_failure(
            "ERROR: connect to 192.0.2.1:443 failed, then [2001:db8::1]:443, (192.0.2.2:80): reset",
        )
        .to_string();

        assert!(!error.contains("192.0.2"), "{error}");
        assert!(!error.contains("2001:db8"), "{error}");
        assert!(error.contains("failed"), "{error}");
    }

    #[test]
    fn failure_messages_are_bounded() {
        let ResolveError::Failed(message) = classify_failure(&"x ".repeat(1000)) else {
            panic!("expected Failed");
        };

        assert_eq!(message.chars().count(), MAX_MESSAGE_CHARS);
    }

    #[tokio::test]
    async fn missing_executable_is_reported() {
        let tool = YtDlp::new("/definitely/not/yt-dlp", None);

        let error = tool.resolve(&video(), 140, IpFamily::V4).await.unwrap_err();

        assert!(matches!(error, ResolveError::Failed(message) if message.contains("not started")));
    }

    #[tokio::test]
    async fn failing_executable_is_reported() {
        let tool = YtDlp::new("false", None);

        let error = tool.stream_info(&video(), IpFamily::V4).await.unwrap_err();

        assert!(matches!(error, ResolveError::Failed(_)));
    }

    #[tokio::test]
    async fn output_that_is_not_json_is_reported() {
        let tool = YtDlp::new("echo", None);

        let error = tool.resolve(&video(), 140, IpFamily::V4).await.unwrap_err();

        assert_eq!(
            error,
            ResolveError::Failed("unreadable yt-dlp output".to_string())
        );
    }
}

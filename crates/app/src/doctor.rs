//! Diagnostics: tools, routes, the active family and URL resolution per family (FR-C2).

use crate::family::{FamilyController, FamilySwitch};
use dalsegno_core::stream::{IpFamily, IpPolicy, VideoId};
use dalsegno_io::tools::{find_program, version};
use dalsegno_io::ytdlp::Resolver;
use serde::Serialize;

const TOOLS: [(&str, &str); 3] = [
    ("yt-dlp", "--version"),
    ("ffmpeg", "-version"),
    ("ffprobe", "-version"),
];

/// One external tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolStatus {
    /// Tool name.
    pub name: String,
    /// Where it was found.
    pub path: Option<String>,
    /// Its version line, or why that could not be read.
    pub version: Option<String>,
}

/// URL resolution over one family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolutionCheck {
    /// Family tried.
    pub family: IpFamily,
    /// Whether a stream lookup worked.
    pub resolved: bool,
    /// The failure, when it did not.
    pub error: Option<String>,
}

/// The `doctor` report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DoctorReport {
    /// External tools.
    pub tools: Vec<ToolStatus>,
    /// Whether the host has a route to global IPv6 addresses.
    pub has_ipv6_route: bool,
    /// Configured policy.
    pub policy: IpPolicy,
    /// Family in use.
    pub active_family: IpFamily,
    /// Most recent switch, in this process.
    pub last_switch: Option<FamilySwitch>,
    /// URL resolution per family, when a stream was given.
    pub resolution: Vec<ResolutionCheck>,
}

/// Inspects the host, and tries URL resolution when a stream is given.
pub async fn run_doctor<R: Resolver>(
    resolver: &R,
    family: &FamilyController,
    has_ipv6_route: bool,
    stream: Option<&VideoId>,
) -> DoctorReport {
    let mut tools = Vec::new();
    for (name, flag) in TOOLS {
        tools.push(inspect_tool(name, flag).await);
    }
    let mut resolution = Vec::new();
    if let Some(video) = stream {
        for checked in [IpFamily::V6, IpFamily::V4] {
            resolution.push(check_resolution(resolver, video, checked).await);
        }
    }
    DoctorReport {
        tools,
        has_ipv6_route,
        policy: family.policy(),
        active_family: family.active(),
        last_switch: family.last_switch(),
        resolution,
    }
}

async fn inspect_tool(name: &str, flag: &str) -> ToolStatus {
    let path = find_program(name);
    let version = match &path {
        Some(program) => Some(match version(program, flag).await {
            Ok(line) => line,
            Err(error) => error.to_string(),
        }),
        None => None,
    };
    ToolStatus {
        name: name.to_string(),
        path: path.map(|program| program.display().to_string()),
        version,
    }
}

async fn check_resolution<R: Resolver>(
    resolver: &R,
    video: &VideoId,
    family: IpFamily,
) -> ResolutionCheck {
    let result = resolver.stream_info(video, family).await;
    ResolutionCheck {
        family,
        resolved: result.is_ok(),
        error: result.err().map(|error| error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dalsegno_core::redact::Redacted;
    use dalsegno_io::ytdlp::{ResolveError, StreamInfo};

    struct V4OnlyResolver;

    impl Resolver for V4OnlyResolver {
        async fn resolve(
            &self,
            _video: &VideoId,
            _itag: u32,
            _family: IpFamily,
        ) -> Result<Redacted<String>, ResolveError> {
            Err(ResolveError::BotCheck)
        }

        async fn stream_info(
            &self,
            _video: &VideoId,
            family: IpFamily,
        ) -> Result<StreamInfo, ResolveError> {
            match family {
                IpFamily::V6 => Err(ResolveError::BotCheck),
                IpFamily::V4 => Ok(StreamInfo {
                    title: String::new(),
                    channel: None,
                    channel_id: None,
                    thumbnail: None,
                    formats: Vec::new(),
                }),
            }
        }
    }

    #[tokio::test]
    async fn reports_every_tool_and_the_family_state() {
        let family = FamilyController::new(IpPolicy::V4Only, false);

        let report = run_doctor(&V4OnlyResolver, &family, false, None).await;

        let names: Vec<&str> = report.tools.iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(names, ["yt-dlp", "ffmpeg", "ffprobe"]);
        assert_eq!(report.policy, IpPolicy::V4Only);
        assert_eq!(report.active_family, IpFamily::V4);
        assert!(report.resolution.is_empty());
    }

    #[tokio::test]
    async fn resolves_per_family_when_a_stream_is_given() {
        let family = FamilyController::new(IpPolicy::V6First, true);
        let video = VideoId::parse("abcDEF123_-").unwrap();

        let report = run_doctor(&V4OnlyResolver, &family, true, Some(&video)).await;

        assert_eq!(
            report.resolution,
            [
                ResolutionCheck {
                    family: IpFamily::V6,
                    resolved: false,
                    error: Some("the origin asked for a bot check".to_string()),
                },
                ResolutionCheck {
                    family: IpFamily::V4,
                    resolved: true,
                    error: None,
                },
            ]
        );
    }

    #[tokio::test]
    async fn missing_tools_are_listed_without_a_path() {
        let status = inspect_tool("definitely-not-a-tool", "--version").await;

        assert_eq!(status.path, None);
        assert_eq!(status.version, None);
    }
}

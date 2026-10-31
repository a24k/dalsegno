//! Identity of a stream and the IP family used to reach it.

use serde::Serialize;
use std::fmt;
use std::str::FromStr;

const VIDEO_ID_LENGTH: usize = 11;
const TITLE_SEPARATORS: [&str; 4] = [" -", " –", " |", "【"];

/// Why a video ID or URL was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("not a video ID or a recognised video URL")]
pub struct InvalidVideoId;

/// A validated video ID, safe to pass to child processes (FR-S1).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct VideoId(String);

impl VideoId {
    /// Parses a bare ID, or extracts one from a watch, live or short URL.
    pub fn parse(input: &str) -> Result<Self, InvalidVideoId> {
        let trimmed = input.trim();
        let candidate = if trimmed.contains('/') {
            id_from_url(trimmed).ok_or(InvalidVideoId)?
        } else {
            trimmed
        };
        is_valid_id(candidate)
            .then(|| Self(candidate.to_string()))
            .ok_or(InvalidVideoId)
    }

    /// Returns the ID as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the canonical watch URL.
    pub fn watch_url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.0)
    }
}

impl fmt::Display for VideoId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn is_valid_id(candidate: &str) -> bool {
    candidate.len() == VIDEO_ID_LENGTH
        && candidate
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn id_from_url(url: &str) -> Option<&str> {
    let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let (host, rest) = after_scheme.split_once('/')?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    match host.trim_start_matches("www.").trim_start_matches("m.") {
        "youtu.be" => Some(path),
        "youtube.com" if path == "watch" => {
            query.split('&').find_map(|pair| pair.strip_prefix("v="))
        }
        "youtube.com" => path.strip_prefix("live/"),
        _ => None,
    }
}

/// One of the two address families.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum IpFamily {
    /// IPv4.
    V4,
    /// IPv6.
    V6,
}

impl IpFamily {
    /// Returns the other family.
    pub fn other(self) -> Self {
        match self {
            Self::V4 => Self::V6,
            Self::V6 => Self::V4,
        }
    }
}

impl fmt::Display for IpFamily {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::V4 => "ipv4",
            Self::V6 => "ipv6",
        })
    }
}

/// Which families may be used, and in what order (FR-A4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IpPolicy {
    /// Prefer IPv6, fall back to IPv4.
    V6First,
    /// Prefer IPv4, fall back to IPv6.
    V4First,
    /// IPv6 only.
    V6Only,
    /// IPv4 only.
    V4Only,
}

/// The text was not one of the policy names.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("expected v6-first, v4-first, v6-only or v4-only")]
pub struct InvalidIpPolicy;

impl FromStr for IpPolicy {
    type Err = InvalidIpPolicy;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "v6-first" => Ok(Self::V6First),
            "v4-first" => Ok(Self::V4First),
            "v6-only" => Ok(Self::V6Only),
            "v4-only" => Ok(Self::V4Only),
            _ => Err(InvalidIpPolicy),
        }
    }
}

/// Proposes a short name from a stream title, for a person to confirm (FR-S2).
pub fn propose_short_name(title: &str) -> String {
    let trimmed = title.trim();
    let end = TITLE_SEPARATORS
        .iter()
        .filter_map(|separator| trimmed.find(separator))
        .filter(|position| *position > 0)
        .min()
        .unwrap_or(trimmed.len());
    trimmed[..end].trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "abcDEF123_-";

    #[test]
    fn accepts_a_bare_id() {
        assert_eq!(VideoId::parse(ID).unwrap().as_str(), ID);
    }

    #[test]
    fn extracts_the_id_from_urls() {
        for url in [
            format!("https://www.youtube.com/watch?v={ID}"),
            format!("https://www.youtube.com/watch?list=x&v={ID}&t=1"),
            format!("https://m.youtube.com/watch?v={ID}"),
            format!("https://youtu.be/{ID}?si=abc"),
            format!("https://www.youtube.com/live/{ID}"),
        ] {
            assert_eq!(VideoId::parse(&url).unwrap().as_str(), ID, "{url}");
        }
    }

    #[test]
    fn rejects_bad_input() {
        for input in [
            "",
            "short",
            "abcDEF123_-x",
            "abc DEF123_",
            "--rm-rf-all;",
            "https://example.com/watch?v=abcDEF123_-",
            "https://www.youtube.com/watch?v=short",
            "https://www.youtube.com/watch",
        ] {
            assert_eq!(VideoId::parse(input), Err(InvalidVideoId), "{input}");
        }
    }

    #[test]
    fn builds_the_watch_url() {
        assert_eq!(
            VideoId::parse(ID).unwrap().watch_url(),
            format!("https://www.youtube.com/watch?v={ID}")
        );
    }

    #[test]
    fn other_family_flips() {
        assert_eq!(IpFamily::V4.other(), IpFamily::V6);
        assert_eq!(IpFamily::V6.other(), IpFamily::V4);
    }

    #[test]
    fn parses_policy_names() {
        assert_eq!("v6-first".parse(), Ok(IpPolicy::V6First));
        assert_eq!("v4-first".parse(), Ok(IpPolicy::V4First));
        assert_eq!("v6-only".parse(), Ok(IpPolicy::V6Only));
        assert_eq!("v4-only".parse(), Ok(IpPolicy::V4Only));
        assert_eq!("ipv4".parse::<IpPolicy>(), Err(InvalidIpPolicy));
    }

    #[test]
    fn proposes_short_names() {
        assert_eq!(propose_short_name("Example TV -An Archive-"), "Example TV");
        assert_eq!(propose_short_name("Example TV | 24/7"), "Example TV");
        assert_eq!(propose_short_name("Example TV【Live】"), "Example TV");
        assert_eq!(propose_short_name("  Example TV  "), "Example TV");
        assert_eq!(propose_short_name("-An Archive-"), "-An Archive-");
    }
}

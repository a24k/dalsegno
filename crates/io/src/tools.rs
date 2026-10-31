//! External tools and host facts: program lookup, versions, ffprobe and ffmpeg checks.

use std::env;
use std::ffi::{OsStr, OsString};
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

const TIMEOUT: Duration = Duration::from_secs(120);
/// A global address used only to ask the routing table. Connecting UDP sends no packet.
const IPV6_ROUTE_PROBE: &str = "[2001:4860:4860::8888]:443";

/// Why a tool could not do its work.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ToolError {
    /// The tool could not be started, or ran too long.
    #[error("{0} did not run: {1}")]
    NotRun(String, String),
    /// The tool reported failure for this input.
    #[error("{0} exited with failure")]
    Failed(String),
}

/// Finds an executable in `PATH`.
pub fn find_program(name: &str) -> Option<PathBuf> {
    find_in(name, env::var_os("PATH").as_deref())
}

fn find_in(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    env::split_paths(path?)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

/// Returns the first output line of `<program> <flag>`.
pub async fn version(program: &Path, flag: &str) -> Result<String, ToolError> {
    let stdout = run(program, &[OsString::from(flag)]).await?;
    Ok(first_line(&stdout))
}

fn first_line(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Returns whether the host has a route to global IPv6 addresses.
pub fn has_ipv6_route() -> bool {
    UdpSocket::bind("[::]:0").is_ok_and(|socket| socket.connect(IPV6_ROUTE_PROBE).is_ok())
}

/// Returns one flag per video packet of `file`, true for a keyframe.
pub async fn video_keyframes(ffprobe: &Path, file: &Path) -> Result<Vec<bool>, ToolError> {
    let args = [
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "packet=flags",
        "-of",
        "csv=p=0",
    ]
    .map(OsString::from)
    .into_iter()
    .chain([file.as_os_str().to_owned()])
    .collect::<Vec<_>>();
    let stdout = run(ffprobe, &args).await?;
    Ok(parse_packet_flags(&String::from_utf8_lossy(&stdout)))
}

fn parse_packet_flags(csv: &str) -> Vec<bool> {
    csv.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.starts_with('K'))
        .collect()
}

/// Copies the video of `input` into an mp4 without re-encoding.
pub async fn remux_video_to_mp4(
    ffmpeg: &Path,
    input: &Path,
    output: &Path,
) -> Result<(), ToolError> {
    let args: Vec<OsString> = vec![
        "-v".into(),
        "error".into(),
        "-y".into(),
        "-i".into(),
        input.into(),
        "-map".into(),
        "0:v:0".into(),
        "-c".into(),
        "copy".into(),
        "-f".into(),
        "mp4".into(),
        output.into(),
    ];
    run(ffmpeg, &args).await.map(|_| ())
}

async fn run(program: &Path, args: &[OsString]) -> Result<Vec<u8>, ToolError> {
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
    let output = tokio::time::timeout(TIMEOUT, run)
        .await
        .map_err(|_| ToolError::NotRun(name.clone(), "timed out".to_string()))?
        .map_err(|error| ToolError::NotRun(name.clone(), error.kind().to_string()))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(ToolError::Failed(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_a_program_in_the_search_path() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("tool");
        fs::write(&program, b"").unwrap();
        let path = env::join_paths(["/definitely/not/here".as_ref(), dir.path()]).unwrap();

        assert_eq!(find_in("tool", Some(&path)), Some(program));
    }

    #[test]
    fn reports_a_missing_program() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(find_in("tool", Some(dir.path().as_os_str())), None);
        assert_eq!(find_in("tool", None), None);
    }

    #[test]
    fn directories_are_not_programs() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("tool")).unwrap();

        assert_eq!(find_in("tool", Some(dir.path().as_os_str())), None);
    }

    #[test]
    fn keeps_the_first_line_of_a_version() {
        assert_eq!(
            first_line(b"ffmpeg version 8.0\nbuilt with\n"),
            "ffmpeg version 8.0"
        );
        assert_eq!(first_line(b""), "");
    }

    #[test]
    fn reads_keyframe_flags() {
        assert_eq!(
            parse_packet_flags("K__\n___\n\n__D,\nK__,\n"),
            [true, false, false, true]
        );
    }

    #[tokio::test]
    async fn version_reports_program_output() {
        let line = version(Path::new("echo"), "1.2.3").await.unwrap();

        assert_eq!(line, "1.2.3");
    }

    #[tokio::test]
    async fn failing_tools_are_named() {
        let error = version(Path::new("false"), "--version").await.unwrap_err();

        assert_eq!(error, ToolError::Failed("false".to_string()));
    }

    #[tokio::test]
    async fn missing_tools_are_reported() {
        let error = video_keyframes(Path::new("/not/ffprobe"), Path::new("/not/file"))
            .await
            .unwrap_err();

        assert!(matches!(error, ToolError::NotRun(name, _) if name == "ffprobe"));
    }
}

//! Container reader checks against real fragments. Skipped without fixtures.

use std::path::{Path, PathBuf};
use std::process::Command;

use dalsegno_core::container::{parse_mp4_fragment, parse_webm_start_time};
use dalsegno_test_support::require_fixtures;

const CONSECUTIVE: usize = 200;
const PROBED: usize = 5;
const TOLERANCE_SECONDS: f64 = 1e-3;

fn first_fragments(dir: &Path, count: usize) -> Vec<PathBuf> {
    let first = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| entry.unwrap().file_name().to_str()?.parse::<u64>().ok())
        .min()
        .unwrap();
    (first..first + count as u64)
        .map(|sq| dir.join(sq.to_string()))
        .collect()
}

fn ffprobe_start_time(path: &Path) -> f64 {
    let output = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=start_time"])
        .args(["-of", "default=nw=1:nk=1"])
        .arg(path)
        .output()
        .expect("ffprobe must be installed to run fixture tests");
    String::from_utf8(output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[test]
fn audio_fragments_are_contiguous_and_match_ffprobe() {
    let dir = require_fixtures!().join("frag-140");
    let paths = first_fragments(&dir, CONSECUTIVE);
    let files: Vec<Vec<u8>> = paths.iter().map(|p| std::fs::read(p).unwrap()).collect();

    let fragments: Vec<_> = files
        .iter()
        .map(|bytes| parse_mp4_fragment(bytes).unwrap())
        .collect();

    for pair in fragments.windows(2) {
        assert_eq!(
            pair[0].start_ticks + pair[0].duration_ticks,
            pair[1].start_ticks
        );
    }
    for (path, fragment) in paths.iter().zip(&fragments).take(PROBED) {
        let expected = ffprobe_start_time(path);
        assert!((fragment.start_time() - expected).abs() < TOLERANCE_SECONDS);
    }
}

#[test]
fn video_start_times_match_ffprobe() {
    let dir = require_fixtures!().join("frag-248");

    for path in first_fragments(&dir, PROBED) {
        let actual = parse_webm_start_time(&std::fs::read(&path).unwrap()).unwrap();
        assert!((actual - ffprobe_start_time(&path)).abs() < TOLERANCE_SECONDS);
    }
}

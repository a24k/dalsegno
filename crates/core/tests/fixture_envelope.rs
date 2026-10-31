//! The envelope pipeline on real fragments. Skipped without `DALSEGNO_FIXTURES`.
//!
//! The Python envelope is a compatibility reference, not a measure of accuracy (spike S4).

use dalsegno_core::container::parse_mp4_fragment;
use dalsegno_core::features::EnvelopeExtractor;
use dalsegno_test_support::{read_npy_f32, require_fixtures};
use std::env;
use std::fs;
use std::path::Path;

const REFERENCE: &str = "env-1031.npy";
/// The first fragment of the audio that `REFERENCE` was built from.
const FIRST_SQ: u64 = 2_528_515;
/// Ten minutes by default. `DALSEGNO_ENVELOPE_FRAGMENTS` extends it, up to 34,560.
const DEFAULT_FRAGMENTS: u64 = 120;
const FRAGMENT_COUNT_ENV: &str = "DALSEGNO_ENVELOPE_FRAGMENTS";
const RESTART_FRAGMENTS: usize = 40;
const RESTART_AT: usize = 20;
/// A restart loses the decoder's noise substitution state, which shows in the last bits.
const RESTART_TOLERANCE: f32 = 1e-5;

const MIN_CORRELATION: f64 = 0.9999;
const MAX_MEAN_DIFFERENCE: f64 = 0.001;
const MAX_P99_DIFFERENCE: f64 = 0.005;

fn read_fragments(directory: &Path, count: u64) -> Vec<Vec<u8>> {
    (FIRST_SQ..FIRST_SQ + count)
        .map(|sq| fs::read(directory.join(sq.to_string())).expect("contiguous fixture fragments"))
        .collect()
}

fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let mean = |values: &[f32]| {
        values.iter().map(|value| f64::from(*value)).sum::<f64>() / values.len() as f64
    };
    let (mean_a, mean_b) = (mean(a), mean(b));
    let (mut product, mut square_a, mut square_b) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (f64::from(*x) - mean_a, f64::from(*y) - mean_b);
        product += x * y;
        square_a += x * x;
        square_b += y * y;
    }
    product / (square_a * square_b).sqrt()
}

#[test]
fn envelope_passes_the_s4_compatibility_check() {
    let fixtures = require_fixtures!();
    let count = env::var(FRAGMENT_COUNT_ENV)
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(DEFAULT_FRAGMENTS);
    let reference = read_npy_f32(&fixtures.join(REFERENCE));
    let files = read_fragments(&fixtures.join("frag-140"), count);
    let first = parse_mp4_fragment(&files[0]).unwrap();
    // Python's bins start at the first sample, so this comparison puts bin zero there.
    let mut extractor = EnvelopeExtractor::with_grid_origin(&first, first.start_ticks).unwrap();

    let mut bins = Vec::new();
    for file in &files {
        let fragment = parse_mp4_fragment(file).unwrap();
        bins.extend(extractor.process(&fragment).unwrap().values);
    }

    let reference = &reference[..bins.len()];
    let correlation = correlation(&bins, reference);
    let mut differences: Vec<f64> = bins
        .iter()
        .zip(reference)
        .map(|(a, b)| f64::from(a - b).abs())
        .collect();
    differences.sort_by(f64::total_cmp);
    let mean = differences.iter().sum::<f64>() / differences.len() as f64;
    let p99 = differences[differences.len() * 99 / 100];
    let max = differences[differences.len() - 1];
    println!(
        "bins {} correlation {correlation:.7} mean {mean:.5} p99 {p99:.5} max {max:.5}",
        bins.len()
    );
    assert!(correlation >= MIN_CORRELATION, "correlation {correlation}");
    assert!(mean <= MAX_MEAN_DIFFERENCE, "mean {mean}");
    assert!(p99 <= MAX_P99_DIFFERENCE, "p99 {p99}");
}

#[test]
fn a_restart_matches_one_pass_except_for_the_noise_generator() {
    let fixtures = require_fixtures!();
    let files = read_fragments(&fixtures.join("frag-140"), RESTART_FRAGMENTS as u64);
    let fragments: Vec<_> = files
        .iter()
        .map(|file| parse_mp4_fragment(file).unwrap())
        .collect();

    let mut whole = EnvelopeExtractor::new(&fragments[0]).unwrap();
    let rows: Vec<_> = fragments
        .iter()
        .map(|fragment| whole.process(fragment).unwrap())
        .collect();

    let end_bin = rows[RESTART_AT - 1].first_bin + rows[RESTART_AT - 1].values.len() as u64;
    let mut resumed = EnvelopeExtractor::new(&fragments[RESTART_AT - 1]).unwrap();
    resumed.skip_before(end_bin);
    resumed.process(&fragments[RESTART_AT - 1]).unwrap();

    for (fragment, expected) in fragments[RESTART_AT..].iter().zip(&rows[RESTART_AT..]) {
        let resumed_row = resumed.process(fragment).unwrap();
        assert_eq!(resumed_row.first_bin, expected.first_bin);
        assert_eq!(resumed_row.values.len(), expected.values.len());
        for (a, b) in resumed_row.values.iter().zip(&expected.values) {
            assert!((a - b).abs() <= RESTART_TOLERANCE, "{a} {b}");
        }
    }
}

#[test]
fn positions_follow_the_measured_fragment_times() {
    let fixtures = require_fixtures!();
    let files = read_fragments(&fixtures.join("frag-140"), 3);
    let fragments: Vec<_> = files
        .iter()
        .map(|file| parse_mp4_fragment(file).unwrap())
        .collect();
    let mut extractor = EnvelopeExtractor::new(&fragments[0]).unwrap();

    for pair in fragments.windows(2) {
        extractor.process(&pair[0]).unwrap();
        assert_eq!(extractor.end_tick(), pair[1].start_ticks);
    }
}

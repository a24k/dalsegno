//! Analysis commands on files, which need neither the service nor a library (FR-N11).

use crate::error::AcquireError;
use dalsegno_core::analysis::{
    Diff, DiffConfig, KnownClip, LagRun, PeriodConfig, PeriodEstimate, Prepared, Series, Signal,
    TrackConfig, VersionCycle, compare, detect_signals, estimate_period, locate_clip, summarise,
    track,
};
use dalsegno_core::container::parse_mp4_fragment;
use dalsegno_core::features::{BIN_RATE, EnvelopeExtractor};
use dalsegno_io::npy::{read_f32, write_f32};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

const CLIP_THRESHOLD: f32 = 0.8;

fn load(path: &Path) -> Result<Prepared, AcquireError> {
    let values = read_f32(path).map_err(|error| AcquireError::Envelope(error.to_string()))?;
    Ok(Prepared::new(&Series::complete(0.0, BIN_RATE, values)))
}

/// What was written by [`build_envelope`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EnvelopeReport {
    /// Fragments read.
    pub fragments: u64,
    /// Bins written.
    pub bins: u64,
}

/// Builds an envelope from numbered fragment files, with bin zero at the first sample.
pub fn build_envelope(
    directory: &Path,
    first: u64,
    last: u64,
    output: &Path,
) -> Result<EnvelopeReport, AcquireError> {
    if first > last {
        return Err(AcquireError::InvalidRequest("the range is reversed"));
    }
    let failure = |error: String| AcquireError::Envelope(error);
    let mut bins = Vec::new();
    let mut extractor = None;
    for sq in first..=last {
        let bytes = fs::read(directory.join(sq.to_string()))?;
        let fragment =
            parse_mp4_fragment(&bytes).map_err(|error| failure(format!("{sq}: {error}")))?;
        let current = match &mut extractor {
            Some(current) => current,
            slot @ None => slot.insert(
                EnvelopeExtractor::with_grid_origin(&fragment, fragment.start_ticks)
                    .map_err(|error| failure(error.to_string()))?,
            ),
        };
        let processed = current
            .process(&fragment)
            .map_err(|error| failure(format!("{sq}: {error}")))?;
        bins.extend(processed.values);
    }
    write_f32(output, &bins).map_err(|error| failure(error.to_string()))?;
    Ok(EnvelopeReport {
        fragments: last - first + 1,
        bins: bins.len() as u64,
    })
}

/// The outcome of `period`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PeriodReport {
    /// Seconds, when one was found.
    pub period: Option<f64>,
    /// References that agreed.
    pub agreeing: usize,
    /// Whether a match also exists at twice the period.
    pub confirmed_by_double: bool,
    /// The longest period the history could show, when none was found (D-03).
    pub testable_up_to: Option<f64>,
}

/// Estimates the period of an envelope file.
pub fn period(path: &Path, config: &PeriodConfig) -> Result<PeriodReport, AcquireError> {
    Ok(match estimate_period(&load(path)?, config) {
        PeriodEstimate::Found {
            period,
            agreeing,
            confirmed_by_double,
        } => PeriodReport {
            period: Some(period),
            agreeing,
            confirmed_by_double,
            testable_up_to: None,
        },
        PeriodEstimate::NoPeriod { testable_up_to } => PeriodReport {
            period: None,
            agreeing: 0,
            confirmed_by_double: false,
            testable_up_to: Some(testable_up_to),
        },
    })
}

/// The outcome of `scan`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScanReport {
    /// Stretches of the lag.
    pub runs: Vec<LagRun>,
    /// What they mean.
    pub signals: Vec<Signal>,
}

/// Tracks the lag through an envelope file from `from` seconds, or from one period in.
pub fn scan(path: &Path, period: f64, from: Option<f64>) -> Result<ScanReport, AcquireError> {
    let data = load(path)?;
    let config = TrackConfig::default();
    let start = from.unwrap_or(period);
    let points = track(&data, period, start..data.end_time(), &config, false);
    let runs = summarise(&points, &config);
    let signals = detect_signals(&runs, period, &config);
    Ok(ScanReport { runs, signals })
}

/// What `diff` compares: the last cycle of each file.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffRequest {
    /// Envelope of the current version.
    pub current: PathBuf,
    /// Its period.
    pub period: f64,
    /// Envelope of the previous version.
    pub against: PathBuf,
    /// Its period.
    pub against_period: f64,
    /// Envelopes of older versions, with their periods, newest first.
    pub older: Vec<(PathBuf, f64)>,
    /// Envelope of a recurring clip.
    pub known: Option<PathBuf>,
}

/// Compares the last cycle of one envelope file with the last cycles of others.
pub fn diff(request: &DiffRequest) -> Result<Diff, AcquireError> {
    let config = DiffConfig::default();
    let current = load(&request.current)?;
    let start = last_cycle_start(&current, request.period, &config)?;
    let mut sources = vec![(load(&request.against)?, request.against_period)];
    for (path, period) in &request.older {
        sources.push((load(path)?, *period));
    }
    let versions = sources
        .iter()
        .enumerate()
        .map(|(index, (data, period))| {
            Ok(VersionCycle {
                id: index as i64,
                data,
                start: last_cycle_start(data, *period, &config)?,
                period: *period,
            })
        })
        .collect::<Result<Vec<_>, AcquireError>>()?;
    let known: Vec<KnownClip> = match &request.known {
        Some(path) => {
            let end = start + request.period;
            locate_clip(&current, &load(path)?, start..end, CLIP_THRESHOLD)
        }
        None => Vec::new(),
    };
    Ok(compare(
        &current,
        start,
        request.period,
        &versions,
        &known,
        &config,
    ))
}

fn last_cycle_start(
    data: &Prepared,
    period: f64,
    config: &DiffConfig,
) -> Result<f64, AcquireError> {
    let start = data.end_time() - period - config.window;
    if start < 0.0 {
        return Err(AcquireError::InvalidRequest(
            "an envelope is shorter than one cycle",
        ));
    }
    Ok(start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dalsegno_test_support::require_fixtures;
    use std::path::PathBuf;

    const PERIOD: f64 = 2500.0;

    /// Noise with memory, which matches only itself.
    fn noise(seed: u64, seconds: f64) -> Vec<f32> {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut level = 0.0;
        (0..(seconds * BIN_RATE) as usize)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let value = (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5;
                level = 0.8 * level + 1.6 * value;
                8.0 + level
            })
            .collect()
    }

    fn file(dir: &Path, name: &str, values: &[f32]) -> PathBuf {
        let path = dir.join(name);
        write_f32(&path, values).unwrap();
        path
    }

    #[test]
    fn period_is_found_in_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "env.npy", &noise(1, PERIOD).repeat(4));
        let config = PeriodConfig {
            min_period: 600.0,
            max_period: 4000.0,
            reference_seconds: 120.0,
            reference_spacing: 120.0,
            ..PeriodConfig::default()
        };

        let report = period(&path, &config).unwrap();

        assert!((report.period.unwrap() - PERIOD).abs() < 0.2);
        assert!(report.confirmed_by_double);
    }

    #[test]
    fn a_file_with_no_loop_says_how_far_it_could_test() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path(), "env.npy", &noise(2, 5000.0));

        let report = period(&path, &PeriodConfig::default()).unwrap();

        assert_eq!(report.period, None);
        assert!(report.testable_up_to.is_some());
    }

    #[test]
    fn scan_reports_a_change() {
        let dir = tempfile::tempdir().unwrap();
        let cycle = noise(3, PERIOD);
        let insert = noise(4, 900.0);
        let values = [&cycle[..], &cycle[..10_000], &insert, &cycle[10_000..]].concat();
        let path = file(dir.path(), "env.npy", &values);

        let report = scan(&path, PERIOD, None).unwrap();

        assert!(report.signals.iter().any(|signal| matches!(
            signal,
            Signal::LagShift { delta, .. } if (delta - 900.0).abs() < 0.2
        )));
    }

    #[test]
    fn diff_compares_the_last_cycles_and_labels_known_clips() {
        let dir = tempfile::tempdir().unwrap();
        let old = noise(5, PERIOD);
        let clip = noise(6, 400.0);
        let new = [&old[..10_000], &clip, &old[10_000..]].concat();
        let request = DiffRequest {
            current: file(dir.path(), "new.npy", &new.repeat(2)),
            period: PERIOD + 400.0,
            against: file(dir.path(), "old.npy", &old.repeat(2)),
            against_period: PERIOD,
            older: Vec::new(),
            known: Some(file(dir.path(), "clip.npy", &clip)),
        };

        let result = diff(&request).unwrap();

        assert!(
            result.checks.iter().all(|check| check.passed),
            "{:?}",
            result.checks
        );
        assert!(
            result
                .regions
                .iter()
                .any(|region| region.class == dalsegno_core::analysis::RegionClass::Known),
            "{:?}",
            result.regions
        );
    }

    #[test]
    fn diff_needs_a_whole_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let short = file(dir.path(), "short.npy", &noise(7, 100.0));
        let request = DiffRequest {
            current: short.clone(),
            period: PERIOD,
            against: short,
            against_period: PERIOD,
            older: Vec::new(),
            known: None,
        };

        assert!(matches!(
            diff(&request),
            Err(AcquireError::InvalidRequest(_))
        ));
    }

    #[test]
    fn a_broken_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.npy");
        fs::write(&path, b"garbage").unwrap();

        assert!(matches!(
            period(&path, &PeriodConfig::default()),
            Err(AcquireError::Envelope(_))
        ));
    }

    #[test]
    fn a_reversed_range_is_refused() {
        assert!(matches!(
            build_envelope(Path::new("/nowhere"), 2, 1, Path::new("/nowhere/out")),
            Err(AcquireError::InvalidRequest(_))
        ));
    }

    #[test]
    fn envelopes_are_built_from_numbered_files() {
        let fixtures = require_fixtures!();
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("env.npy");

        let report =
            build_envelope(&fixtures.join("frag-140"), 2_528_515, 2_528_520, &output).unwrap();

        assert_eq!(report.fragments, 6);
        assert_eq!(read_f32(&output).unwrap().len() as u64, report.bins);
    }

    #[test]
    fn a_missing_fragment_stops_the_build() {
        let dir = tempfile::tempdir().unwrap();

        assert!(matches!(
            build_envelope(dir.path(), 1, 2, &dir.path().join("out.npy")),
            Err(AcquireError::Store(_))
        ));
    }
}

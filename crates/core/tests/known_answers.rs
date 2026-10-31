//! The known answers of the reference stream. Skipped without `DALSEGNO_FIXTURES`.
//!
//! `DALSEGNO_ENVELOPES` points at another directory of envelopes under the same names, so the
//! same answers can be demanded of the in-process envelope. That is what shows it is good
//! enough for analysis (roadmap, M3 exit).

use dalsegno_core::analysis::{
    DiffConfig, PeriodConfig, PeriodEstimate, Prepared, RegionClass, Series, Signal, TrackConfig,
    VersionCycle, compare, detect_signals, estimate_period, summarise, track,
};
use dalsegno_test_support::{read_npy_f32, require_fixtures};
use std::env;
use std::path::PathBuf;

const ENVELOPES_ENV: &str = "DALSEGNO_ENVELOPES";
const RATE: f64 = 10.0;
const HOUR: f64 = 3600.0;

/// Stream time of the first sample of each envelope, from the first fragment of its range.
const RECENT_START: f64 = 12_252_581.581_497;
const LATEST_START: f64 = 12_642_615.054_512;

/// The first 33 hours of `env-recent` end just before the 10/26 insertion.
const HOURS_BEFORE_INSERTION: f64 = 33.0;
const VERSION_1024_PERIOD: f64 = 37_649.07;
const AFTER_INSERTION_PERIOD: f64 = 38_376.7;
const VERSION_1031_PERIOD: f64 = 42_244.7;
const INSERTION: f64 = 727.6;
const ADDITION_A: f64 = 455.0;
const ADDITION_B: f64 = 3_414.0;

fn envelope(name: &str) -> Option<Vec<f32>> {
    let fixtures = dalsegno_test_support::fixtures_dir()?;
    let directory = env::var_os(ENVELOPES_ENV).map_or(fixtures, PathBuf::from);
    let path = directory.join(name);
    path.exists().then(|| read_npy_f32(&path))
}

fn prepare(start: f64, values: &[f32]) -> Prepared {
    Prepared::new(&Series::complete(start, RATE, values.to_vec()))
}

fn period_of(data: &Prepared) -> f64 {
    match estimate_period(data, &PeriodConfig::default()) {
        PeriodEstimate::Found {
            period, agreeing, ..
        } => {
            println!("period {period} from {agreeing} references");
            period
        }
        PeriodEstimate::NoPeriod { testable_up_to } => {
            panic!("no period, testable up to {testable_up_to}")
        }
    }
}

fn assert_near(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{actual} is not within {tolerance} of {expected}"
    );
}

#[test]
fn initial_version_period() {
    require_fixtures!();
    // Only the Python envelope reaches this far back: the fragments are gone.
    let Some(values) = envelope("env.npy") else {
        return;
    };

    let data = prepare(0.0, &values[..(48.0 * HOUR * RATE) as usize]);

    assert_near(period_of(&data), 38_910.0, 1.0);
}

#[test]
fn version_of_10_24_period() {
    require_fixtures!();
    let values = envelope("env-recent.npy").expect("env-recent.npy");

    let end = (HOURS_BEFORE_INSERTION * HOUR * RATE) as usize;
    let data = prepare(RECENT_START, &values[..end]);

    assert_near(period_of(&data), 37_648.7, 1.0);
}

#[test]
fn period_after_the_10_26_insertion() {
    require_fixtures!();
    let values = envelope("env-recent.npy").expect("env-recent.npy");

    let data = prepare(RECENT_START, &values);

    assert_near(period_of(&data), AFTER_INSERTION_PERIOD, 1.0);
}

#[test]
fn the_10_26_insertion_is_detected_with_its_size() {
    require_fixtures!();
    let values = envelope("env-recent.npy").expect("env-recent.npy");
    let data = prepare(RECENT_START, &values);
    let config = TrackConfig::default();
    let from = RECENT_START + HOURS_BEFORE_INSERTION * HOUR - 1800.0;

    let points = track(
        &data,
        VERSION_1024_PERIOD,
        from..data.end_time() - config.window,
        &config,
        false,
    );
    let runs = summarise(&points, &config);
    let signals = detect_signals(&runs, VERSION_1024_PERIOD, &config);

    println!("{signals:#?}");
    let shift = signals
        .iter()
        .find_map(|signal| match signal {
            Signal::LagShift { delta, .. } => Some(*delta),
            _ => None,
        })
        .expect("a lag shift");
    assert_near(shift, INSERTION, 2.0);
}

#[test]
fn period_of_the_10_31_version() {
    require_fixtures!();
    let values = envelope("env-1031.npy").expect("env-1031.npy");

    let data = prepare(LATEST_START, &values);

    assert_near(period_of(&data), VERSION_1031_PERIOD, 1.0);
}

#[test]
fn comparison_finds_both_additions_of_10_31() {
    require_fixtures!();
    let older = envelope("env-recent.npy").expect("env-recent.npy");
    let newer = envelope("env-1031.npy").expect("env-1031.npy");
    let previous = prepare(RECENT_START, &older);
    let current = prepare(LATEST_START, &newer);
    let config = DiffConfig::default();
    let versions = [VersionCycle {
        id: 1,
        data: &previous,
        start: previous.end_time() - AFTER_INSERTION_PERIOD - config.window,
        period: AFTER_INSERTION_PERIOD,
    }];
    let start = current.end_time() - VERSION_1031_PERIOD - config.window;

    let diff = compare(
        &current,
        start,
        VERSION_1031_PERIOD,
        &versions,
        &[],
        &config,
    );

    for region in &diff.regions {
        println!(
            "{:?} {:.0}..{:.0} ({:.0} s) shift {:?}",
            region.class,
            region.start,
            region.end,
            region.end - region.start,
            region.shift
        );
    }
    println!("shift changes {:?}", diff.shift_changes);
    println!("checks {:#?}", diff.checks);
    assert_near(diff.shift_changes.iter().sum(), 3_868.0, 5.0);
    let near = |value: f64| {
        diff.shift_changes
            .iter()
            .any(|change| (change - value).abs() <= 5.0)
    };
    assert!(near(ADDITION_A), "{:?}", diff.shift_changes);
    assert!(near(ADDITION_B), "{:?}", diff.shift_changes);
    assert!(
        diff.regions
            .iter()
            .any(|region| region.class == RegionClass::New)
    );
}

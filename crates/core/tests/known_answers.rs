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

/// The first 32 hours of `env-recent` end just before the 10/26 insertion.
const HOURS_BEFORE_INSERTION: f64 = 32.0;
/// These hours include the whole of the insertion.
const HOURS_AFTER_INSERTION: f64 = 33.0;
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
    let inside_existing = |start: f64, end: f64| {
        EXISTING_STRETCHES
            .iter()
            .any(|(first, last)| start >= *first && end <= *last)
    };
    let false_regions: Vec<_> = diff
        .regions
        .iter()
        .filter(|region| region.class == RegionClass::New)
        .filter(|region| inside_existing(region.start, region.end))
        .collect();
    assert_eq!(
        false_regions,
        Vec::<&dalsegno_core::analysis::Region>::new()
    );
}

/// Stretches of the 10/31 cycle that match the previous version, in stream time.
const EXISTING_STRETCHES: [(f64, f64); 4] = [
    (12_773_400.0, 12_780_000.0),
    (12_781_500.0, 12_789_700.0),
    (12_794_700.0, 12_799_500.0),
    (12_801_000.0, 12_815_000.0),
];
const TRIAL_SPACING: f64 = 700.0;
/// Addition B, which is audio the previous version does not have.
const NEW_AUDIO: (f64, f64) = (12_790_100.0, 12_793_500.0);
const SOURCE_STEP: f64 = 137.0;
/// Audio from elsewhere in the cycle, which the previous version does have.
const SOURCE_DISTANCE: f64 = 15_000.0;

/// Overwrites stretches of real audio with other real audio of the same length, which makes
/// net-zero replacements with a known answer. Returns (found, trials, false regions).
fn replacement_trial(seconds: f64, with_new_audio: bool) -> Option<(usize, usize, usize)> {
    let older = envelope("env-recent.npy")?;
    let newer = envelope("env-1031.npy")?;
    let previous = prepare(RECENT_START, &older);
    let index = |time: f64| ((time - LATEST_START) * RATE) as usize;
    let mut edited = newer.clone();
    let mut planted = Vec::new();
    for (first, last) in EXISTING_STRETCHES {
        let mut time = first + TRIAL_SPACING / 2.0;
        while time + seconds < last {
            let (at, length) = (index(time), (seconds * RATE) as usize);
            let span = NEW_AUDIO.1 - NEW_AUDIO.0 - seconds;
            let source = if with_new_audio {
                index(NEW_AUDIO.0 + (planted.len() as f64 * SOURCE_STEP) % span)
            } else {
                at - (SOURCE_DISTANCE * RATE) as usize
            };
            edited[at..at + length].copy_from_slice(&newer[source..source + length]);
            planted.push((time, time + seconds));
            time += TRIAL_SPACING;
        }
    }
    let current = prepare(LATEST_START, &edited);
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
    let new_regions: Vec<_> = diff
        .regions
        .iter()
        .filter(|region| region.class == RegionClass::New)
        .filter(|region| {
            EXISTING_STRETCHES
                .iter()
                .any(|(a, b)| region.start >= *a && region.end <= *b)
        })
        .collect();
    let overlap = |region: &dalsegno_core::analysis::Region, plant: &(f64, f64)| {
        region.end.min(plant.1) - region.start.max(plant.0)
    };
    let found = planted
        .iter()
        .filter(|plant| {
            new_regions
                .iter()
                .any(|region| overlap(region, plant) >= seconds / 2.0)
        })
        .count();
    let stray = new_regions
        .iter()
        .filter(|region| !planted.iter().any(|plant| overlap(region, plant) > 0.0))
        .count();
    Some((found, planted.len(), stray))
}

/// (seconds, share found at least) for a replacement with audio the previous version lacks.
/// Below 90 s a replacement increasingly disappears into its surroundings.
const NEW_AUDIO_DETECTION: [(f64, f64); 4] =
    [(90.0, 0.85), (120.0, 0.9), (150.0, 0.95), (211.0, 0.95)];

#[test]
fn net_zero_replacements_on_real_audio() {
    require_fixtures!();
    for (seconds, wanted) in NEW_AUDIO_DETECTION {
        let Some((found, trials, stray)) = replacement_trial(seconds, true) else {
            return;
        };
        println!("{seconds:.0} s: found {found}/{trials}, stray {stray}");
        assert!(found as f64 >= wanted * trials as f64, "{found}/{trials}");
        assert_eq!(stray, 0);
    }
}

#[test]
fn tracking_of_net_zero_replacements_on_real_audio() {
    require_fixtures!();
    let Some(newer) = envelope("env-1031.npy") else {
        return;
    };
    let index = |time: f64| ((time - LATEST_START) * RATE) as usize;
    for seconds in [120.0, 211.0, 400.0] {
        let mut edited = newer.clone();
        let mut planted = Vec::new();
        for (first, last) in EXISTING_STRETCHES {
            let mut time = first + TRIAL_SPACING / 2.0;
            while time + seconds < last {
                let (at, length) = (index(time), (seconds * RATE) as usize);
                let span = NEW_AUDIO.1 - NEW_AUDIO.0 - seconds;
                let source = index(NEW_AUDIO.0 + (planted.len() as f64 * SOURCE_STEP) % span);
                edited[at..at + length].copy_from_slice(&newer[source..source + length]);
                planted.push((time, time + seconds));
                time += TRIAL_SPACING;
            }
        }
        let data = prepare(LATEST_START, &edited);
        let config = TrackConfig::default();
        let points = track(
            &data,
            VERSION_1031_PERIOD,
            data.end_time() - VERSION_1031_PERIOD..data.end_time() - config.window,
            &config,
            false,
        );
        let runs = summarise(&points, &config);
        let signals = detect_signals(&runs, VERSION_1031_PERIOD, &config);
        let found = planted
            .iter()
            .filter(|plant| {
                signals.iter().any(|signal| match signal {
                    Signal::Unmatched { start, end } => *start < plant.1 && *end > plant.0,
                    _ => false,
                })
            })
            .count();
        println!("tracking {seconds:.0} s: found {found}/{}", planted.len());
    }
}

/// Compares the cycle ending at `end` with the one before it, in one envelope.
fn new_regions_between_cycles(
    values: &[f32],
    start_time: f64,
    end: f64,
    period: f64,
) -> Vec<(f64, f64)> {
    let data = prepare(start_time, values);
    let config = DiffConfig::default();
    let versions = [VersionCycle {
        id: 1,
        data: &data,
        start: end - 2.0 * period - config.window,
        period,
    }];
    let start = end - period - config.window;
    let diff = compare(&data, start, period, &versions, &[], &config);
    println!("{:?}", diff.regions);
    diff.regions
        .iter()
        .filter(|region| region.class == RegionClass::New)
        .map(|region| (region.start, region.end))
        .collect()
}

// These pairs played no part in choosing the thresholds, so they test them.
#[test]
fn consecutive_cycles_of_the_10_31_version_have_nothing_new() {
    require_fixtures!();
    let values = envelope("env-1031.npy").expect("env-1031.npy");
    let end = LATEST_START + values.len() as f64 / RATE;

    let found = new_regions_between_cycles(&values, LATEST_START, end, VERSION_1031_PERIOD);

    assert_eq!(found, []);
}

#[test]
fn the_10_26_insertion_is_found_by_comparing_cycles() {
    require_fixtures!();
    let values = envelope("env-recent.npy").expect("env-recent.npy");
    let end = RECENT_START + HOURS_AFTER_INSERTION * HOUR;

    let found = new_regions_between_cycles(&values, RECENT_START, end, 37_648.7);

    // "About 16 minutes in and about 4 minutes out" in the measured facts.
    assert_eq!(found.len(), 1, "{found:?}");
    assert_near(found[0].1 - found[0].0, 16.0 * 60.0, 60.0);
}

#[test]
fn consecutive_cycles_of_the_10_24_version_have_nothing_new() {
    require_fixtures!();
    let values = envelope("env-recent.npy").expect("env-recent.npy");
    let end = RECENT_START + HOURS_BEFORE_INSERTION * HOUR;

    let found = new_regions_between_cycles(&values, RECENT_START, end, 37_648.7);

    assert_eq!(found, []);
}

//! Tracking on synthetic streams.

use super::super::series::Series;
use super::super::synthetic::{content, prepared};
use super::*;

const P: f64 = 3000.0;

fn config() -> TrackConfig {
    TrackConfig {
        min_shift_seconds: 300.0,
        ..TrackConfig::default()
    }
}

fn lag(value: f64) -> Match {
    Match::Lag {
        lag: value,
        score: 0.9,
    }
}

fn points(values: &[Match]) -> Vec<TrackPoint> {
    values
        .iter()
        .enumerate()
        .map(|(index, found)| TrackPoint {
            time: index as f64 * 30.0,
            found: *found,
        })
        .collect()
}

fn empty() -> Prepared {
    prepared(&[])
}

fn signals_of(data: &Prepared, from: f64) -> (Vec<LagRun>, Vec<Signal>) {
    let config = config();
    let tracked = track(data, P, from..data.end_time() - 60.0, &config, false);
    let runs = summarise(&tracked, &config);
    let signals = detect_signals(data, &runs, P, &config);
    (runs, signals)
}

fn shifts(signals: &[Signal]) -> Vec<f64> {
    signals
        .iter()
        .filter_map(|signal| match signal {
            Signal::LagShift { delta, .. } => Some(*delta),
            _ => None,
        })
        .collect()
}

#[test]
fn bounds_keep_zero_and_double_out_of_range() {
    assert_eq!(lag_bounds(36_000.0), 25_200.0..46_800.0);
    assert_eq!(lag_bounds(3_000.0), 1_500.0..4_500.0);
}

#[test]
fn a_steady_loop_raises_nothing() {
    let data = prepared(&content(1, P).repeat(3));

    let (runs, signals) = signals_of(&data, P + 10.0);

    assert_eq!(signals, []);
    assert_eq!(runs.len(), 1);
    assert!((runs[0].lag.unwrap() - P).abs() < 0.2);
}

#[test]
fn growth_settles_at_the_new_lag() {
    let cycle = content(2, P);
    let insert = content(3, 500.0);
    let data = prepared(&[&cycle[..], &cycle[..12_000], &insert, &cycle[12_000..]].concat());

    let (_, signals) = signals_of(&data, P + 10.0);

    let found = shifts(&signals);
    assert_eq!(found.len(), 1, "{signals:?}");
    assert!((found[0] - 500.0).abs() < 0.2, "{found:?}");
    assert!(
        signals
            .iter()
            .any(|signal| matches!(signal, Signal::Unmatched { .. }))
    );
}

#[test]
fn shrinkage_is_seen_below_the_period() {
    let cycle = content(4, P);
    let shorter: Vec<f32> = [&cycle[..12_000], &cycle[20_000..]].concat();
    let data = prepared(&[&cycle[..], &shorter[..], &shorter[..]].concat());

    let (_, signals) = signals_of(&data, P + 1300.0);

    let found = shifts(&signals);
    assert!(!found.is_empty(), "{signals:?}");
    assert!((found[0] + 800.0).abs() < 0.2, "{found:?}");
}

#[test]
fn a_net_zero_replacement_is_caught_by_unmatched_points() {
    let cycle = content(5, P);
    let replacement = content(6, 211.0);
    let data = prepared(&[&cycle[..], &cycle[..12_000], &replacement, &cycle[14_110..]].concat());

    let (_, signals) = signals_of(&data, P + 10.0);

    assert_eq!(shifts(&signals), Vec::<f64>::new());
    assert!(
        signals
            .iter()
            .any(|signal| matches!(signal, Signal::Unmatched { .. })),
        "{signals:?}"
    );
}

#[test]
fn flat_music_that_matches_poorly_is_not_new() {
    let flat: Vec<f32> = (0..3_000)
        .map(|index| 9.0 + ((index * 7919) % 13) as f32 * 0.01)
        .collect();
    let cycle = [&content(20, 1200.0)[..], &flat, &content(21, 1500.0)].concat();
    let shifted: Vec<f32> = flat.iter().rev().copied().collect();
    let later = [&cycle[..12_000], &shifted, &cycle[15_000..]].concat();
    let data = prepared(&[&cycle[..], &later[..]].concat());

    let (runs, signals) = signals_of(&data, P + 10.0);

    assert_eq!(signals, [], "{runs:?}");
}

#[test]
fn a_replaced_video_signals_only_that_video() {
    let gap = vec![0.0; 20];
    let flat = |seed: u64| -> Vec<f32> {
        content(seed, 600.0)
            .iter()
            .map(|value| 9.0 + (value - 8.0) * 0.25)
            .collect()
    };
    let song = content(20, 200.0);
    let other = content(21, 200.0);
    let cycle = |middle: &[f32], noise: u64| {
        [
            content(22, 1000.0).as_slice(),
            &gap,
            &flat(noise),
            &gap,
            middle,
            &gap,
            &flat(noise + 1),
            &gap,
            content(23, 1000.0).as_slice(),
        ]
        .concat()
    };
    let old = cycle(&song, 30);
    let new = cycle(&other, 40);
    let period = old.len() as f64 / 10.0;
    let data = prepared(&[old, new].concat());
    let config = TrackConfig::default();
    let points = track(
        &data,
        period,
        period..2.0 * period - config.window,
        &config,
        false,
    );
    let runs = summarise(&points, &config);

    let signals = detect_signals(&data, &runs, period, &config);

    let song_start = period + 1602.0 + 2.0;
    let [Signal::Unmatched { start, end }] = signals[..] else {
        panic!("{signals:?}");
    };
    assert!((start - song_start).abs() < 5.0, "{start}");
    assert!(
        (end + config.window - (song_start + 200.0)).abs() < 5.0,
        "{end}"
    );
}

#[test]
fn a_replacement_that_matches_elsewhere_is_caught() {
    let cycle = content(22, P);
    let later = [&cycle[..6_000], &cycle[20_000..22_110], &cycle[8_110..]].concat();
    let data = prepared(&[&cycle[..], &later[..]].concat());

    let (_, signals) = signals_of(&data, P + 10.0);

    assert!(
        signals
            .iter()
            .any(|signal| matches!(signal, Signal::Unmatched { .. })),
        "{signals:?}"
    );
}

#[test]
fn a_replacement_still_playing_at_the_end_is_caught() {
    let cycle = content(23, P);
    let later = [&cycle[..12_000], &content(24, 300.0)[..]].concat();
    let data = prepared(&[&cycle[..], &later[..]].concat());

    let (_, signals) = signals_of(&data, P + 10.0);

    assert!(
        signals
            .iter()
            .any(|signal| matches!(signal, Signal::Unmatched { .. })),
        "{signals:?}"
    );
}

#[test]
fn a_lag_that_settles_and_returns_is_not_new_content() {
    let data = prepared(&content(25, 6000.0));
    let config = config();
    let run = |start: f64, points: usize, lag: f64| LagRun {
        start,
        end: start + (points - 1) as f64 * config.step,
        kind: RunKind::Lag,
        lag: Some(lag),
        points,
    };
    let runs = [
        run(3200.0, 20, P),
        run(3800.0, 30, P + 500.0),
        run(4700.0, 20, P),
    ];

    let signals = detect_signals(&data, &runs, P, &config);

    assert!(
        !signals
            .iter()
            .any(|signal| matches!(signal, Signal::Unmatched { .. })),
        "{signals:?}"
    );
}

#[test]
fn a_recording_gap_raises_nothing_a_cycle_later() {
    let cycle = content(7, P);
    let mut series = Series::complete(0.0, 10.0, cycle.repeat(3));
    series.valid[12_000..15_000].fill(false);
    let data = Prepared::new(&series);

    let (runs, signals) = signals_of(&data, P + 10.0);

    assert_eq!(signals, [], "{runs:?}");
    assert!(runs.iter().any(|run| run.kind == RunKind::NoData));
}

#[test]
fn repeated_material_shows_as_an_outlier_and_not_a_shift() {
    let found = [lag(P), lag(P), lag(P), lag(944.0), lag(P), lag(P)];
    let config = config();

    let runs = summarise(&points(&found), &config);

    assert_eq!(runs[1].kind, RunKind::Outlier);
    assert_eq!(
        shifts(&detect_signals(&empty(), &runs, P, &config)),
        Vec::<f64>::new()
    );
}

#[test]
fn a_long_run_is_never_an_outlier() {
    let found = vec![lag(P + 100.0); 8];

    assert_eq!(summarise(&points(&found), &config())[0].kind, RunKind::Lag);
}

#[test]
fn short_unmatched_stretches_do_not_signal() {
    let found = [
        lag(P),
        Match::NoMatch,
        Match::NoMatch,
        Match::NoMatch,
        lag(P),
    ];
    let config = config();

    let runs = summarise(&points(&found), &config);

    assert_eq!(detect_signals(&empty(), &runs, P, &config), []);
}

#[test]
fn no_data_is_never_evidence() {
    let found = [
        lag(P),
        Match::NoData,
        Match::NoData,
        Match::NoData,
        Match::NoData,
        Match::NoData,
        lag(P),
    ];
    let config = config();

    let runs = summarise(&points(&found), &config);

    assert_eq!(detect_signals(&empty(), &runs, P, &config), []);
}

#[test]
fn a_long_outage_is_not_lost_track() {
    let mut found = vec![lag(P)];
    found.extend(vec![Match::NoData; 80]);
    found.push(lag(P));
    let config = config();

    let runs = summarise(&points(&found), &config);

    assert_eq!(detect_signals(&empty(), &runs, P, &config), []);
}

#[test]
fn a_long_stretch_without_a_stable_run_loses_track() {
    let mut found = vec![lag(P)];
    found.extend(vec![Match::NoMatch; 80]);
    found.push(lag(P));
    let config = config();

    let runs = summarise(&points(&found), &config);
    let signals = detect_signals(&empty(), &runs, P, &config);

    assert!(
        signals
            .iter()
            .any(|signal| matches!(signal, Signal::LostTrack { .. })),
        "{signals:?}"
    );
}

#[test]
fn losing_track_at_the_end_of_the_data_is_reported() {
    let mut found = vec![lag(P)];
    found.extend(vec![Match::NoMatch; 80]);
    let config = config();

    let runs = summarise(&points(&found), &config);
    let signals = detect_signals(&empty(), &runs, P, &config);

    assert!(
        signals
            .iter()
            .any(|signal| matches!(signal, Signal::LostTrack { .. })),
        "{signals:?}"
    );
}

#[test]
fn a_long_insertion_is_found_by_widening() {
    let cycle = content(8, P);
    let insert = content(9, 4000.0);
    let data = prepared(&[&cycle[..], &cycle[..12_000], &insert, &cycle[12_000..]].concat());
    let config = config();
    let from = 2.0 * P + 4100.0 - 1200.0;

    let normal = track(&data, P, from..from + 300.0, &config, false);
    let wide = track(&data, P, from..from + 300.0, &config, true);

    assert!(
        normal
            .iter()
            .all(|point| !matches!(point.found, Match::Lag { .. }))
    );
    assert!(wide.iter().all(|point| {
        matches!(point.found, Match::Lag { lag, .. } if (lag - (P + 4000.0)).abs() < 0.2)
    }));
}

#[test]
fn narrowing_brackets_an_insertion() {
    let cycle = content(10, P);
    let insert = content(11, 500.0);
    let data = prepared(&[&cycle[..], &cycle[..12_000], &insert, &cycle[12_000..]].concat());

    let bounds = narrow_change(&data, 4000.0..5000.0, P, P + 500.0, &config());

    let (start, end) = (bounds.start.unwrap(), bounds.end.unwrap());
    assert!((start - 4200.0).abs() < 60.0, "{start}");
    assert!((end - 4700.0).abs() < 60.0, "{end}");
}

#[test]
fn a_stretch_with_no_video_standing_out_ends_at_its_last_point() {
    let cycle = content(24, 2000.0);
    let replacement = content(25, 300.0);
    let new = [&cycle[..10_000], &replacement, &cycle[13_000..]].concat();
    let data = prepared(&[cycle.as_slice(), &new].concat());
    let config = TrackConfig::default();
    let points = track(
        &data,
        2000.0,
        2000.0..4000.0 - config.window,
        &config,
        false,
    );
    let runs = summarise(&points, &config);
    let last_unmatched = runs
        .iter()
        .filter(|run| run.kind != RunKind::Lag)
        .map(|run| run.end)
        .fold(f64::MIN, f64::max);

    let signals = detect_signals(&data, &runs, 2000.0, &config);

    let [Signal::Unmatched { end, .. }] = signals[..] else {
        panic!("{signals:?}");
    };
    assert_eq!(end, last_unmatched);
}

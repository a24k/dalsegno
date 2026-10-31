//! Following the lag over time, and turning it into change signals (D-01, D-04, D-05).

use super::ncc::{Correlator, MIN_REFERENCE_DEVIATION, Match, search};
use super::same_audio::is_same_audio;
use super::series::Prepared;
use rayon::prelude::*;
use serde::Serialize;
use std::ops::Range;

const THREE_HOURS: f64 = 3.0 * 3600.0;

/// Tracking parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackConfig {
    /// Seconds between points. 30 s keeps a 3:31 song from slipping between points (D-04).
    pub step: f64,
    /// Window, in seconds.
    pub window: f64,
    /// Score that counts as a match. The measured range is 0.6 to 0.7.
    pub threshold: f32,
    /// Lags within this many seconds belong to one run.
    pub lag_tolerance: f64,
    /// A run of at most this many points with other lags on both sides is an outlier.
    pub outlier_points: usize,
    /// A lag run must last this long to count as a shift, in seconds.
    pub min_shift_seconds: f64,
    /// Points in a stretch before it can signal new content (D-04).
    pub min_unmatched_points: usize,
    /// A lag held for this many points can vouch for the audio around it. This must outlast
    /// repeated material, which matches its other occurrences for as long as it plays.
    pub min_lasting_points: usize,
    /// Seconds without a stable run that count as lost track (D-01).
    pub lost_track_seconds: f64,
}

impl Default for TrackConfig {
    fn default() -> Self {
        Self {
            step: 30.0,
            window: 60.0,
            threshold: 0.65,
            lag_tolerance: 1.0,
            outlier_points: 3,
            min_shift_seconds: 600.0,
            min_unmatched_points: 4,
            min_lasting_points: 10,
            lost_track_seconds: 1800.0,
        }
    }
}

/// The result at one time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackPoint {
    /// Stream time of the window's start.
    pub time: f64,
    /// What the search found.
    pub found: Match,
}

/// What a stretch of points means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunKind {
    /// A steady lag.
    Lag,
    /// A short run between other lags, usually repeated material.
    Outlier,
    /// Audible content that matched nothing.
    Unmatched,
    /// Silence or a recording gap, which is no evidence.
    NoData,
}

/// A stretch of consecutive points that mean the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LagRun {
    /// Stream time of the first point.
    pub start: f64,
    /// Stream time of the last point.
    pub end: f64,
    /// What the stretch is.
    pub kind: RunKind,
    /// The lag, for `Lag` and `Outlier`.
    pub lag: Option<f64>,
    /// Points in the stretch.
    pub points: usize,
}

/// A reason to think the playlist changed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    /// The lag settled at the period plus `delta`.
    LagShift {
        /// When the new lag began.
        start: f64,
        /// When it was last seen.
        end: f64,
        /// Net change in seconds.
        delta: f64,
    },
    /// Audible content matched nothing, including a net-zero replacement.
    Unmatched {
        /// First point.
        start: f64,
        /// Last point.
        end: f64,
    },
    /// No stable run for a long time, so the search should widen.
    LostTrack {
        /// When stability ended.
        start: f64,
        /// When it returned, or the end of the data.
        end: f64,
    },
}

/// Returns the normal lag search range for a period (D-01).
pub fn lag_bounds(period: f64) -> Range<f64> {
    (period / 2.0).max(period - THREE_HOURS)..(1.5 * period).min(period + THREE_HOURS)
}

/// Tracks each step in `times`. `widened` searches every lag the history allows (D-01).
pub fn track(
    data: &Prepared,
    period: f64,
    times: Range<f64>,
    config: &TrackConfig,
    widened: bool,
) -> Vec<TrackPoint> {
    let count = ((times.end - times.start) / config.step).ceil().max(0.0) as usize;
    let lags = if widened {
        config.window..data.end_time() - data.start_time
    } else {
        lag_bounds(period)
    };
    (0..count)
        .into_par_iter()
        .map_init(Correlator::default, |correlator, index| {
            let time = times.start + index as f64 * config.step;
            TrackPoint {
                time,
                found: search(
                    correlator,
                    data,
                    time,
                    config.window,
                    lags.clone(),
                    config.threshold,
                    Some(period),
                ),
            }
        })
        .collect()
}

/// Groups points into runs, and marks isolated short runs as outliers.
pub fn summarise(points: &[TrackPoint], config: &TrackConfig) -> Vec<LagRun> {
    let mut runs: Vec<LagRun> = Vec::new();
    for point in points {
        let (kind, lag) = match point.found {
            Match::NoData => (RunKind::NoData, None),
            Match::NoMatch => (RunKind::Unmatched, None),
            Match::Lag { lag, .. } => (RunKind::Lag, Some(lag)),
        };
        match runs.last_mut() {
            Some(run) if continues(run, kind, lag, config) => {
                run.end = point.time;
                run.points += 1;
            }
            _ => runs.push(LagRun {
                start: point.time,
                end: point.time,
                kind,
                lag,
                points: 1,
            }),
        }
    }
    mark_outliers(&runs, config)
}

fn continues(run: &LagRun, kind: RunKind, lag: Option<f64>, config: &TrackConfig) -> bool {
    let same_lag = match (run.lag, lag) {
        (Some(a), Some(b)) => (a - b).abs() <= config.lag_tolerance,
        (None, None) => true,
        _ => false,
    };
    run.kind == kind && same_lag
}

fn mark_outliers(runs: &[LagRun], config: &TrackConfig) -> Vec<LagRun> {
    let differs = |run: &LagRun, other: Option<&LagRun>| {
        other.is_none_or(|neighbour| {
            neighbour.kind != RunKind::Lag || !continues(neighbour, run.kind, run.lag, config)
        })
    };
    runs.iter()
        .enumerate()
        .map(|(index, run)| {
            let previous = index.checked_sub(1).and_then(|at| runs.get(at));
            let is_short = run.kind == RunKind::Lag && run.points <= config.outlier_points;
            if is_short && differs(run, previous) && differs(run, runs.get(index + 1)) {
                LagRun {
                    kind: RunKind::Outlier,
                    ..*run
                }
            } else {
                *run
            }
        })
        .collect()
}

/// Turns runs into signals. A score drop alone never fires (FR-N4).
pub fn detect_signals(
    data: &Prepared,
    runs: &[LagRun],
    period: f64,
    config: &TrackConfig,
) -> Vec<Signal> {
    let mut signals = Vec::new();
    let mut stable_until: Option<f64> = runs.first().map(|run| run.start);
    for run in runs {
        match (run.kind, run.lag) {
            (RunKind::Lag, Some(lag)) => {
                if stable_until.is_some_and(|since| run.start - since >= config.lost_track_seconds)
                {
                    signals.push(Signal::LostTrack {
                        start: stable_until.unwrap_or(run.start),
                        end: run.start,
                    });
                }
                stable_until = Some(run.end);
                let delta = lag - period;
                let lasted = run.end - run.start >= config.min_shift_seconds;
                if lasted && delta.abs() > config.lag_tolerance {
                    signals.push(Signal::LagShift {
                        start: run.start,
                        end: run.end,
                        delta,
                    });
                }
            }
            // Silence and outages are no evidence, so they do not count towards lost track.
            (RunKind::NoData, _) => {
                let skipped = run.end - run.start + config.step;
                stable_until = stable_until.map(|since| since + skipped);
            }
            _ => {}
        }
    }
    if let (Some(since), Some(last)) = (stable_until, runs.last())
        && last.end - since >= config.lost_track_seconds
    {
        signals.push(Signal::LostTrack {
            start: since,
            end: last.end,
        });
    }
    signals.extend(unmatched_signals(data, runs, config));
    signals
}

/// Finds content that differs from the cycle before.
///
/// A window of flat music often matches somewhere by chance, and matches poorly where it should,
/// so single points say little. Instead, everything after a lasting lag is judged as a whole at
/// that lag, up to where the same lag lasts again.
fn unmatched_signals(data: &Prepared, runs: &[LagRun], config: &TrackConfig) -> Vec<Signal> {
    let lasting =
        |run: &LagRun| run.kind == RunKind::Lag && run.points >= config.min_lasting_points;
    let same_lag = |a: &LagRun, b: &LagRun| {
        a.lag
            .zip(b.lag)
            .is_some_and(|(x, y)| (x - y).abs() <= config.lag_tolerance)
    };
    let mut signals = Vec::new();
    let first = runs.iter().position(lasting).unwrap_or(runs.len());
    signals.extend(raw_signals(&runs[..first], config));
    let mut at = first;
    while at < runs.len() {
        let anchor = runs[at];
        let again = (at + 1..runs.len()).find(|index| {
            let run = &runs[*index];
            lasting(run) && same_lag(run, &anchor)
        });
        let next = (at + 1..runs.len()).find(|index| lasting(&runs[*index]));
        match (again, next) {
            (Some(end), _) => {
                signals.extend(judged_signals(data, &runs[at + 1..end], &anchor, config));
                at = end;
            }
            // The lag moved on, so the audio before the new lag was never in the old cycle.
            (None, Some(end)) => {
                signals.extend(raw_signals(&runs[at + 1..end], config));
                at = end;
            }
            (None, None) => {
                signals.extend(judged_signals(data, &runs[at + 1..], &anchor, config));
                at = runs.len();
            }
        }
    }
    signals
}

/// Judges each part of `runs` between stretches of no data, at the lag of `anchor`.
fn judged_signals(
    data: &Prepared,
    runs: &[LagRun],
    anchor: &LagRun,
    config: &TrackConfig,
) -> Vec<Signal> {
    let Some(lag) = anchor.lag else {
        return Vec::new();
    };
    runs.split(|run| run.kind == RunKind::NoData)
        .flat_map(|part| {
            let (Some(first), Some(last)) = (part.first(), part.last()) else {
                return Vec::new();
            };
            if part.iter().map(|run| run.points).sum::<usize>() < config.min_unmatched_points {
                return Vec::new();
            }
            let seconds = last.end - first.start + config.window;
            match is_same_audio(data, first.start, data, first.start - lag, seconds) {
                Some(true) => Vec::new(),
                Some(false) => vec![Signal::Unmatched {
                    start: first.start,
                    end: last.end,
                }],
                None => raw_signals(part, config),
            }
        })
        .collect()
}

/// Signals every long unmatched run, for audio with no lag to judge it by.
fn raw_signals(runs: &[LagRun], config: &TrackConfig) -> Vec<Signal> {
    runs.iter()
        .filter(|run| run.kind == RunKind::Unmatched && run.points >= config.min_unmatched_points)
        .map(|run| Signal::Unmatched {
            start: run.start,
            end: run.end,
        })
        .collect()
}

/// Where a change begins and ends, within the window size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChangeBounds {
    /// When the old period stopped matching.
    pub start: Option<f64>,
    /// When the new period started matching for good.
    pub end: Option<f64>,
}

/// Compares the old and new period at 5-second steps over `times` (±30–60 s).
pub fn narrow_change(
    data: &Prepared,
    times: Range<f64>,
    old_period: f64,
    new_period: f64,
    config: &TrackConfig,
) -> ChangeBounds {
    const STEP: f64 = 5.0;
    let count = ((times.end - times.start) / STEP).ceil().max(0.0) as usize;
    let moments = (0..count).map(|index| times.start + index as f64 * STEP);
    let mut correlator = Correlator::default();
    let mut breaks = |time: &f64, period: f64| {
        correlation_at(&mut correlator, data, *time, period, config.window)
            .is_some_and(|score| score < config.threshold)
    };
    let start = moments.clone().find(|time| breaks(time, old_period));
    let end = moments
        .rev()
        .find(|time| breaks(time, new_period))
        .map(|time| time + STEP);
    ChangeBounds { start, end }
}

fn correlation_at(
    correlator: &mut Correlator,
    data: &Prepared,
    time: f64,
    lag: f64,
    window: f64,
) -> Option<f32> {
    let length = data.samples(window);
    let start = data.index_of(time)?;
    let earlier = start.checked_sub(data.samples(lag))?;
    if start + length > data.len() || data.stats(start, length).1 < MIN_REFERENCE_DEVIATION {
        return None;
    }
    if !data.has_data(start, length) || !data.has_data(earlier, length) {
        return None;
    }
    correlator
        .scores(data, start..start + length, earlier..earlier + 1)
        .first()
        .copied()
}

#[cfg(test)]
mod tests {
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
        let data =
            prepared(&[&cycle[..], &cycle[..12_000], &replacement, &cycle[14_110..]].concat());

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
}

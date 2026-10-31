//! Following the lag over time, and turning it into change signals (D-01, D-04, D-05).

use super::ncc::{Correlator, MIN_REFERENCE_DEVIATION, Match, search};
use super::same_audio::is_same_audio;
use super::series::Prepared;
use super::videos::split_at_silences;
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
        // A lag that settles elsewhere is a real change, and repeated material only visits.
        let settled = |run: &LagRun| run.end - run.start >= config.min_shift_seconds;
        let again = (at + 1..runs.len())
            .find(|index| {
                let run = &runs[*index];
                lasting(run) && (same_lag(run, &anchor) || settled(run))
            })
            .filter(|index| same_lag(&runs[*index], &anchor));
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
            let seconds = last.end - first.start + config.step;
            match is_same_audio(data, first.start, data, first.start - lag, seconds) {
                Some(true) => Vec::new(),
                Some(false) => differing_videos(data, first.start..last.end, seconds, lag, config),
                None => raw_signals(part, config),
            }
        })
        .collect()
}

/// Signals the videos of a differing stretch that differ, or all of it when none stands out.
fn differing_videos(
    data: &Prepared,
    points: Range<f64>,
    seconds: f64,
    lag: f64,
    config: &TrackConfig,
) -> Vec<Signal> {
    let pieces = split_at_silences(data, points.start, points.start + seconds);
    let differing = pieces.iter().filter(|(start, end)| {
        is_same_audio(data, *start, data, *start - lag, end - start) != Some(true)
    });
    let mut stretches: Vec<(f64, f64)> = Vec::new();
    for (start, end) in differing {
        match stretches.last_mut() {
            Some(last) if last.1 == *start => last.1 = *end,
            _ => stretches.push((*start, *end)),
        }
    }
    let whole_end = points.start + seconds;
    if stretches.is_empty() {
        stretches.push((points.start, whole_end));
    }
    // A signal ends at its last point, which is the start of a window.
    stretches
        .iter()
        .map(|(start, end)| Signal::Unmatched {
            start: *start,
            end: if *end == whole_end {
                points.end
            } else {
                (end - config.window).max(*start)
            },
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
mod tests;

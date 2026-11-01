//! Comparing a cycle against stored versions, and checking the answer (FR-N5, FR-N8).
//!
//! The baseline. Milestone 7 replaces it behind this interface.

use super::ncc::{Correlator, MIN_REFERENCE_DEVIATION};
use super::period::clusters;
use super::series::Prepared;
use super::videos::{nearest_silence, silence_centres};
use edges::{EdgeComparison, refine_edges};
use rayon::prelude::*;
use serde::Serialize;

mod edges;
mod reclaim;

/// A silence this close to the edge of a region belongs to the edge, in seconds.
const EDGE_MARGIN: f64 = 5.0;
/// The name of the check on silences.
const SILENCE_CHECK: &str = "silences recur at the matched shift";
/// How far from the expected place a silence may lie, in seconds. Shifts within a region differ
/// by up to the shift tolerance.
const SILENCE_REACH: f64 = 3.5;
/// Share of silences that may be missing, as level around them can differ between versions.
const SILENCE_TOLERANCE: f64 = 0.1;

/// Comparison parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiffConfig {
    /// Window, in seconds.
    pub window: f64,
    /// Seconds between windows.
    pub step: f64,
    /// Score that counts as a match.
    pub threshold: f32,
    /// Shifts within this many seconds belong to one region.
    pub shift_tolerance: f64,
    /// A shift held for fewer windows is a brief flip. This must outlast repeated material,
    /// which matches its other occurrences for as long as it plays.
    pub min_windows: usize,
    /// How far a self-check may be off, in seconds.
    pub check_tolerance: f64,
    /// A stretch of the old cycle shorter than this is not reported as deleted, in seconds.
    pub min_deleted_seconds: f64,
    /// How far one moved edge can be off, in seconds. Lengths inherit this per edge.
    pub edge_tolerance: f64,
    /// Window for moving a region's edges, in seconds.
    pub edge_window: f64,
    /// Score that keeps an edge moving.
    pub edge_threshold: f64,
    /// A gap between matched regions shorter than this is an edge artefact, in seconds.
    pub min_region_seconds: f64,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            window: 60.0,
            step: 30.0,
            threshold: 0.7,
            shift_tolerance: 3.0,
            min_windows: 10,
            check_tolerance: 5.0,
            min_deleted_seconds: 60.0,
            edge_tolerance: 3.0,
            edge_window: 5.0,
            edge_threshold: 0.8,
            min_region_seconds: 20.0,
        }
    }
}

/// One stored version to search in.
#[derive(Debug, Clone, Copy)]
pub struct VersionCycle<'a> {
    /// Caller's identifier.
    pub id: i64,
    /// Series holding the version's audio.
    pub data: &'a Prepared,
    /// Start of the cycle, in that series' stream time.
    pub start: f64,
    /// The version's period.
    pub period: f64,
}

/// A registered recurring clip (FR-N7).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct KnownClip {
    /// Where the clip occurs in the current cycle, in stream time.
    pub start: f64,
    /// End of that occurrence.
    pub end: f64,
}

/// What a region of the cycle is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegionClass {
    /// In the previous version.
    Existing,
    /// Not in the previous version, but in an older one.
    Revival,
    /// Overlaps a registered clip.
    Known,
    /// Found nowhere.
    New,
    /// Silence or a recording gap, which says nothing either way (D-05).
    NoData,
    /// In the previous version, with no counterpart now.
    Deleted,
}

/// One stretch of the comparison.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Region {
    /// What it is.
    pub class: RegionClass,
    /// Start. For `Deleted`, in the previous version's time.
    pub start: f64,
    /// End.
    pub end: f64,
    /// `time - time in the matched version`, modulo that version's period.
    pub shift: Option<f64>,
    /// The matched version.
    pub version: Option<i64>,
    /// Where the region starts in the matched version.
    pub match_start: Option<f64>,
    /// Whether this is a copy out of the previous version's order, such as repeated material
    /// placed in front of an addition. It counts as inserted.
    pub moved: bool,
}

/// One automatic check of the result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SelfCheck {
    /// What was checked.
    pub name: String,
    /// The expected value.
    pub expected: f64,
    /// The measured value.
    pub actual: f64,
    /// Allowed difference.
    pub tolerance: f64,
    /// Whether it held. A failure asks for human review and never blocks.
    pub passed: bool,
}

/// The result of a comparison.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Diff {
    /// Regions in time order, then deleted ones.
    pub regions: Vec<Region>,
    /// Changes of shift along the regions that keep the previous version's order.
    pub shift_changes: Vec<f64>,
    /// Silences inside new regions, in time order. One region can join several videos, and
    /// which ones belong together is a person's call (FR-N13).
    pub video_boundaries: Vec<f64>,
    /// The checks of FR-N8.
    pub checks: Vec<SelfCheck>,
}

#[derive(Debug, Clone, Copy)]
struct Found {
    version: usize,
    shift: f64,
    position: f64,
    offset: f64,
}

/// What one window found. No data is not a mismatch (D-05).
#[derive(Debug, Clone, Copy)]
enum Window {
    NoData,
    NoMatch,
    Found(Found),
}

#[derive(Debug, Clone, Copy)]
struct Span {
    window: Window,
    first: usize,
    last: usize,
}

/// Compares the cycle of `period` seconds at `start` in `current` against `versions`, the
/// previous version first.
pub fn compare(
    current: &Prepared,
    start: f64,
    period: f64,
    versions: &[VersionCycle<'_>],
    known: &[KnownClip],
    config: &DiffConfig,
) -> Diff {
    let found = match_windows(current, start, period, versions, config);
    let grouped = group(&found, versions, config);
    let bridge = Bridge {
        current,
        versions,
        start,
        config,
    };
    let spans = consolidate(grouped, &bridge);
    let centre = |index: usize| start + index as f64 * config.step + config.window / 2.0;
    let edge = |span: &Span| {
        (
            centre(span.first) - config.step / 2.0,
            centre(span.last) + config.step / 2.0,
        )
    };
    let classified: Vec<Region> = spans
        .iter()
        .map(|span| classify(span, edge(span), versions, known))
        .collect();
    let refined = refine_edges(&classified, &spans, current, versions, config);
    let regions = mark_moved(&refined, versions);
    let shift_changes = shift_changes(&regions, versions);
    let deleted = deleted_regions(&regions, versions, config);
    let inserted: f64 = regions
        .iter()
        .filter(|region| is_inserted(region))
        .map(|region| region.end - region.start)
        .sum();
    let removed: f64 = deleted.iter().map(|region| region.end - region.start).sum();
    let change = versions
        .first()
        .map_or(0.0, |previous| period - previous.period);
    let edges = 2 * (regions.iter().filter(|region| is_inserted(region)).count() + deleted.len());
    let mut checks = vec![
        check(
            "shift changes sum to the period change",
            change,
            shift_changes.iter().sum(),
            config.check_tolerance,
        ),
        check(
            "inserted minus deleted equals the period change",
            change,
            inserted - removed,
            config
                .check_tolerance
                .max(edges as f64 * config.edge_tolerance),
        ),
    ];
    checks.extend(silences_recur(&regions, current, versions));
    let video_boundaries = boundaries_inside_new(&regions, current);
    let mut regions = with_match_starts(&regions, versions);
    regions.extend(deleted);
    Diff {
        regions,
        shift_changes,
        video_boundaries,
        checks,
    }
}

/// Sets where each region starts in its version from its shift, as moving an edge leaves the
/// value from before the move.
fn with_match_starts(regions: &[Region], versions: &[VersionCycle<'_>]) -> Vec<Region> {
    regions
        .iter()
        .map(|region| {
            let cycle = versions
                .iter()
                .find(|cycle| Some(cycle.id) == region.version);
            let found = cycle.zip(region.shift).map(|(cycle, shift)| {
                cycle.start + (region.start - shift - cycle.start).rem_euclid(cycle.period)
            });
            Region {
                match_start: found.or(region.match_start),
                ..*region
            }
        })
        .collect()
}

/// Returns the silences of a region, leaving out the ones at its edges. They are found over the
/// whole region first, as a silence cut by a narrower range has a false middle.
fn silences_inside(region: &Region, current: &Prepared) -> Vec<f64> {
    let inside = (region.start + EDGE_MARGIN)..(region.end - EDGE_MARGIN);
    silence_centres(current, region.start, region.end)
        .into_iter()
        .filter(|time| inside.contains(time))
        .collect()
}

/// Reports the silences inside new regions.
fn boundaries_inside_new(regions: &[Region], current: &Prepared) -> Vec<f64> {
    regions
        .iter()
        .filter(|region| region.class == RegionClass::New)
        .flat_map(|region| silences_inside(region, current))
        .collect()
}

/// Checks that the silences of existing regions are there in the matched version too. This is
/// evidence apart from the envelope matching that placed the regions, as silences are read from
/// the level alone. Audio with no silences to compare gives no check.
fn silences_recur(
    regions: &[Region],
    current: &Prepared,
    versions: &[VersionCycle<'_>],
) -> Option<SelfCheck> {
    let results: Vec<bool> = regions
        .iter()
        .filter(|region| region.class == RegionClass::Existing && !region.moved)
        .filter_map(|region| {
            let cycle = versions
                .iter()
                .find(|cycle| Some(cycle.id) == region.version)?;
            let shift = region.shift?;
            let found = silences_inside(region, current).into_iter().map(|time| {
                let position = cycle.start + (time - shift - cycle.start).rem_euclid(cycle.period);
                nearest_silence(cycle.data, position, SILENCE_REACH).is_some()
            });
            Some(found.collect::<Vec<bool>>())
        })
        .flatten()
        .collect();
    if results.is_empty() {
        return None;
    }
    let matched = results.iter().filter(|found| **found).count();
    Some(check(
        SILENCE_CHECK,
        1.0,
        matched as f64 / results.len() as f64,
        SILENCE_TOLERANCE,
    ))
}

/// Finds each place a recurring clip plays within `range` of `current`.
pub fn locate_clip(
    current: &Prepared,
    clip: &Prepared,
    range: std::ops::Range<f64>,
    threshold: f32,
) -> Vec<KnownClip> {
    let length = clip.len();
    let bounds = (
        current.index_of(range.start),
        current.index_of(range.end),
        current.len().checked_sub(length),
    );
    let (Some(first), Some(end), Some(limit)) = bounds else {
        return Vec::new();
    };
    let last = end.min(limit);
    if first > last || length == 0 {
        return Vec::new();
    }
    if clip.stats(0, length).1 < MIN_REFERENCE_DEVIATION {
        return Vec::new();
    }
    let mut correlator = Correlator::default();
    let scores = correlator.scores_across(clip, 0..length, current, first..last + 1);
    clusters(&scores, threshold)
        .map(|offset| {
            let start = current.time_of(first + offset);
            KnownClip {
                start,
                end: start + length as f64 / current.rate,
            }
        })
        .collect()
}

fn is_inserted(region: &Region) -> bool {
    match region.class {
        RegionClass::Existing => region.moved,
        RegionClass::NoData | RegionClass::Deleted => false,
        RegionClass::Revival | RegionClass::Known | RegionClass::New => true,
    }
}

fn check(name: &str, expected: f64, actual: f64, tolerance: f64) -> SelfCheck {
    SelfCheck {
        name: name.to_string(),
        expected,
        actual,
        tolerance,
        passed: (expected - actual).abs() <= tolerance,
    }
}

fn match_windows(
    current: &Prepared,
    start: f64,
    period: f64,
    versions: &[VersionCycle<'_>],
    config: &DiffConfig,
) -> Vec<Window> {
    let length = current.samples(config.window);
    let count = (period / config.step).floor() as usize;
    (0..count)
        .into_par_iter()
        .map_init(Correlator::default, |correlator, index| {
            let time = start + index as f64 * config.step;
            let Some(at) = current.index_of(time) else {
                return Window::NoData;
            };
            if at + length > current.len()
                || !current.has_data(at, length)
                || current.stats(at, length).1 < MIN_REFERENCE_DEVIATION
            {
                return Window::NoData;
            }
            let found = versions.iter().enumerate().find_map(|(version, cycle)| {
                best_in_version(correlator, current, at, length, time, cycle, config).map(
                    |(position, shift)| Found {
                        version,
                        shift,
                        position,
                        offset: time - position,
                    },
                )
            });
            found.map_or(Window::NoMatch, Window::Found)
        })
        .collect()
}

/// Searches one cycle, extended 10 minutes either side because the comparison spans the seam.
fn best_in_version(
    correlator: &mut Correlator,
    current: &Prepared,
    at: usize,
    length: usize,
    time: f64,
    cycle: &VersionCycle<'_>,
    config: &DiffConfig,
) -> Option<(f64, f64)> {
    const MARGIN: f64 = 600.0;
    let data = cycle.data;
    let first = data.index_of((cycle.start - MARGIN).max(data.start_time))?;
    let last = data
        .index_of(cycle.start + cycle.period + MARGIN)?
        .min(data.len().checked_sub(length)?);
    if first > last {
        return None;
    }
    let scores = correlator.scores_across(current, at..at + length, data, first..last + 1);
    let (offset, score) = scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))?;
    if *score < config.threshold {
        return None;
    }
    let position = data.time_of(first + offset);
    Some((position, (time - position).rem_euclid(cycle.period)))
}

fn circular_distance(a: f64, b: f64, period: f64) -> f64 {
    let difference = (a - b).rem_euclid(period);
    difference.min(period - difference)
}

fn same_key(a: Found, b: Found, versions: &[VersionCycle<'_>], config: &DiffConfig) -> bool {
    a.version == b.version
        && circular_distance(a.shift, b.shift, versions[a.version].period) <= config.shift_tolerance
}

fn group(found: &[Window], versions: &[VersionCycle<'_>], config: &DiffConfig) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    for (index, window) in found.iter().enumerate() {
        match spans.last_mut() {
            Some(span) if same_span(span.window, *window, versions, config) => span.last = index,
            _ => spans.push(Span {
                window: *window,
                first: index,
                last: index,
            }),
        }
    }
    spans
}

fn same_span(a: Window, b: Window, versions: &[VersionCycle<'_>], config: &DiffConfig) -> bool {
    match (a, b) {
        (Window::NoData, Window::NoData) | (Window::NoMatch, Window::NoMatch) => true,
        (Window::Found(a), Window::Found(b)) => same_key(a, b, versions, config),
        _ => false,
    }
}

/// Decides whether a stretch between two equal shifts is the same audio.
struct Bridge<'a> {
    current: &'a Prepared,
    versions: &'a [VersionCycle<'a>],
    start: f64,
    config: &'a DiffConfig,
}

impl Bridge<'_> {
    fn holds(&self, span: &Span, shift: Found) -> bool {
        let config = self.config;
        let time = self.start + span.first as f64 * config.step;
        let seconds = (span.last - span.first) as f64 * config.step + config.window;
        EdgeComparison::at(shift, self.current, self.versions, config).is_same(time, seconds)
    }
}

/// Merges everything between two lasting spans of one shift when the audio there is the same.
///
/// Repeated material can match elsewhere for longer than `min_windows`, so the whole gap is
/// judged at once, and not span by span.
fn bridge_gaps(spans: Vec<Span>, bridge: &Bridge<'_>) -> Vec<Span> {
    let (versions, config) = (bridge.versions, bridge.config);
    let lasting = |span: &Span| match span.window {
        Window::Found(found) if span.last - span.first + 1 >= config.min_windows => Some(found),
        _ => None,
    };
    let mut merged: Vec<Span> = Vec::new();
    let mut index = 0;
    while index < spans.len() {
        let mut current = spans[index];
        index += 1;
        while let Some(found) = lasting(&current) {
            let same_shift = |span: &Span| {
                lasting(span).is_some_and(|other| same_key(found, other, versions, config))
            };
            let Some(next) = (index..spans.len()).find(|at| same_shift(&spans[*at])) else {
                break;
            };
            let gap = Span {
                first: current.last + 1,
                last: spans[next].first.saturating_sub(1),
                ..current
            };
            // Equal shifts can differ by the shift tolerance, so either side may hold the right one.
            let holds = |side: Option<Found>| side.is_some_and(|shift| bridge.holds(&gap, shift));
            if next > index && !holds(Some(found)) && !holds(lasting(&spans[next])) {
                break;
            }
            current.last = spans[next].last;
            index = next + 1;
        }
        merged.push(current);
    }
    merged
}

/// Drops brief flips, and keeps what stayed unmatched apart from what had no data.
fn consolidate(spans: Vec<Span>, bridge: &Bridge<'_>) -> Vec<Span> {
    let (versions, config) = (bridge.versions, bridge.config);
    let width = |span: &Span| span.last - span.first + 1;
    let spans = bridge_gaps(spans, bridge);
    let lasting: Vec<Option<Found>> = spans
        .iter()
        .map(|span| match span.window {
            Window::Found(found) if width(span) >= config.min_windows => Some(found),
            _ => None,
        })
        .collect();
    let mut result: Vec<Span> = Vec::new();
    for (index, span) in spans.iter().enumerate() {
        let previous = lasting[..index].iter().rev().find_map(|found| *found);
        let next = lasting[index + 1..].iter().find_map(|found| *found);
        let around = previous
            .zip(next)
            .filter(|(a, b)| same_key(*a, *b, versions, config))
            .map(|(a, _)| a);
        let window = match (span.window, around) {
            _ if lasting[index].is_some() => span.window,
            (Window::NoData, Some(shift)) => Window::Found(shift),
            (Window::Found(_), _) => Window::NoMatch,
            (other, _) => other,
        };
        match result.last_mut() {
            Some(last) if same_span(last.window, window, versions, config) => last.last = span.last,
            _ => result.push(Span { window, ..*span }),
        }
    }
    result
}

fn classify(
    span: &Span,
    (start, end): (f64, f64),
    versions: &[VersionCycle<'_>],
    known: &[KnownClip],
) -> Region {
    let plain = |class| Region {
        class,
        start,
        end,
        shift: None,
        version: None,
        match_start: None,
        moved: false,
    };
    match span.window {
        Window::Found(found) => Region {
            shift: Some(found.shift),
            version: Some(versions[found.version].id),
            match_start: Some(found.position),
            ..plain(if found.version == 0 {
                RegionClass::Existing
            } else {
                RegionClass::Revival
            })
        },
        Window::NoData => plain(RegionClass::NoData),
        Window::NoMatch if overlaps_known(start, end, known) => plain(RegionClass::Known),
        Window::NoMatch => plain(RegionClass::New),
    }
}

fn overlaps_known(start: f64, end: f64, known: &[KnownClip]) -> bool {
    known.iter().any(|clip| {
        let overlap = end.min(clip.end) - start.max(clip.start);
        overlap >= (end - start) / 2.0
    })
}

/// Region edges are good to about this, in seconds, so overlaps up to it still count as in order.
const ORDER_TOLERANCE: f64 = 60.0;

/// Marks existing regions outside the longest stretch that keeps the old order.
fn mark_moved(regions: &[Region], versions: &[VersionCycle<'_>]) -> Vec<Region> {
    let Some(previous) = versions.first() else {
        return regions.to_vec();
    };
    let existing: Vec<usize> = (0..regions.len())
        .filter(|index| regions[*index].class == RegionClass::Existing)
        .collect();
    let old_start = |index: usize| {
        let region = &regions[index];
        (region.start - region.shift.unwrap_or(0.0)).rem_euclid(previous.period)
    };
    let duration = |index: usize| regions[index].end - regions[index].start;
    let mut best: (f64, Vec<usize>) = (0.0, Vec::new());
    for (position, base) in existing.iter().enumerate() {
        let unrolled =
            |index: usize| (old_start(index) - old_start(*base)).rem_euclid(previous.period);
        let candidates = &existing[position..];
        // The heaviest chain ending at each candidate.
        let mut chains: Vec<(f64, Vec<usize>)> = Vec::with_capacity(candidates.len());
        for (at, index) in candidates.iter().enumerate() {
            let longest = (0..at)
                .filter(|earlier| {
                    let previous_end =
                        unrolled(candidates[*earlier]) + duration(candidates[*earlier]);
                    previous_end <= unrolled(*index) + ORDER_TOLERANCE
                })
                .max_by(|a, b| chains[*a].0.total_cmp(&chains[*b].0));
            let (weight, mut chain) =
                longest.map_or((0.0, Vec::new()), |earlier| chains[earlier].clone());
            chain.push(*index);
            chains.push((weight + duration(*index), chain));
        }
        if let Some(heaviest) = chains.into_iter().max_by(|a, b| a.0.total_cmp(&b.0))
            && heaviest.0 > best.0
        {
            best = heaviest;
        }
    }
    regions
        .iter()
        .enumerate()
        .map(|(index, region)| Region {
            moved: region.class == RegionClass::Existing && !best.1.contains(&index),
            ..*region
        })
        .collect()
}

/// Returns how far the shift moved between regions that keep the old order.
fn shift_changes(regions: &[Region], versions: &[VersionCycle<'_>]) -> Vec<f64> {
    let Some(previous) = versions.first() else {
        return Vec::new();
    };
    let shifts: Vec<f64> = regions
        .iter()
        .filter(|region| region.class == RegionClass::Existing && !region.moved)
        .filter_map(|region| region.shift)
        .collect();
    shifts
        .windows(2)
        .map(|pair| {
            let change = (pair[1] - pair[0]).rem_euclid(previous.period);
            if change > previous.period / 2.0 {
                change - previous.period
            } else {
                change
            }
        })
        .collect()
}

/// Returns what the previous cycle held that no existing region accounts for.
fn deleted_regions(
    regions: &[Region],
    versions: &[VersionCycle<'_>],
    config: &DiffConfig,
) -> Vec<Region> {
    let Some(previous) = versions.first() else {
        return Vec::new();
    };
    let mut covered: Vec<(f64, f64)> = regions
        .iter()
        .filter(|region| region.class == RegionClass::Existing && !region.moved)
        .filter_map(|region| {
            let start = (region.start - region.shift?).rem_euclid(previous.period);
            Some((start, start + region.end - region.start))
        })
        .collect();
    covered.sort_by(|a, b| a.0.total_cmp(&b.0));
    let Some(first) = covered.first().copied() else {
        return Vec::new();
    };
    let mut reach = first.1;
    let mut holes = Vec::new();
    for (start, end) in covered.iter().copied().skip(1) {
        if start > reach {
            holes.push((reach, start));
        }
        reach = reach.max(end);
    }
    if first.0 + previous.period > reach {
        holes.push((reach, first.0 + previous.period));
    }
    holes
        .into_iter()
        .filter(|(start, end)| end - start >= config.min_deleted_seconds)
        .map(|(start, end)| Region {
            class: RegionClass::Deleted,
            start,
            end,
            shift: None,
            version: Some(previous.id),
            match_start: None,
            moved: false,
        })
        .collect()
}

#[cfg(test)]
mod tests;

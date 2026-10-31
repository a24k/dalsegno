//! Comparing a cycle against stored versions, and checking the answer (FR-N5, FR-N8).
//!
//! The baseline. Milestone 7 replaces it behind this interface.

use super::ncc::{Correlator, MIN_REFERENCE_DEVIATION};
use super::period::clusters;
use super::series::Prepared;
use rayon::prelude::*;
use serde::Serialize;

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
    /// Unmatched windows between equal shifts that are bridged, as quiet scenes match poorly.
    /// More than this is a replacement, which keeps the shift the same.
    pub max_bridged_unmatched: usize,
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
            max_bridged_unmatched: 5,
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
    let spans = consolidate(group(&found, versions, config), versions, config);
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
    let mut regions = mark_moved(&refined, versions);
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
    let checks = vec![
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
    regions.extend(deleted);
    Diff {
        regions,
        shift_changes,
        checks,
    }
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

/// Drops brief flips, then joins what they separated.
fn consolidate(spans: Vec<Span>, versions: &[VersionCycle<'_>], config: &DiffConfig) -> Vec<Span> {
    let width = |span: &Span| span.last - span.first + 1;
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
        let window = match (span.window, previous, next) {
            (_, _, _) if lasting[index].is_some() => span.window,
            (Window::NoMatch, Some(a), Some(b))
                if width(span) <= config.max_bridged_unmatched
                    && same_key(a, b, versions, config) =>
            {
                Window::Found(a)
            }
            (Window::NoData | Window::Found(_), Some(a), Some(b))
                if same_key(a, b, versions, config) =>
            {
                Window::Found(a)
            }
            (Window::Found(_), _, _) => Window::NoMatch,
            (other, _, _) => other,
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

/// Moves the edges of matched regions from window precision to about a second.
///
/// A window straddling an edge matches nothing, so raw regions stop short of the truth.
fn refine_edges(
    regions: &[Region],
    spans: &[Span],
    current: &Prepared,
    versions: &[VersionCycle<'_>],
    config: &DiffConfig,
) -> Vec<Region> {
    let moved: Vec<Region> = regions
        .iter()
        .zip(spans)
        .map(|(region, span)| match span.window {
            Window::Found(found) => {
                let comparison = EdgeComparison {
                    current,
                    earlier: versions[found.version].data,
                    offset: found.offset,
                    config,
                };
                Region {
                    start: comparison.move_start(region.start),
                    end: comparison.move_end(region.end),
                    ..*region
                }
            }
            Window::NoData | Window::NoMatch => *region,
        })
        .collect();
    settle(&moved, config)
}

/// Makes neighbouring regions meet, and drops the slivers left between matched regions.
fn settle(regions: &[Region], config: &DiffConfig) -> Vec<Region> {
    let is_matched = |region: &Region| region.shift.is_some();
    let mut settled: Vec<Region> = Vec::new();
    for (index, region) in regions.iter().enumerate() {
        let next = regions.get(index + 1);
        let previous = index.checked_sub(1).map(|at| regions[at]);
        if !is_matched(region) {
            let start = previous.map_or(region.start, |neighbour| neighbour.end);
            let end = next.map_or(region.end, |neighbour| neighbour.start);
            let is_sliver = end - start < config.min_region_seconds;
            if !is_sliver || previous.is_none() || next.is_none() {
                settled.push(Region {
                    start,
                    end: end.max(start),
                    ..*region
                });
            }
            continue;
        }
        let mut current = *region;
        if let Some(last) = settled.last_mut()
            && is_matched(last)
        {
            let meeting = (last.end + current.start) / 2.0;
            last.end = meeting;
            current.start = meeting;
        }
        settled.push(current);
    }
    settled
}

struct EdgeComparison<'a> {
    current: &'a Prepared,
    earlier: &'a Prepared,
    offset: f64,
    config: &'a DiffConfig,
}

impl EdgeComparison<'_> {
    fn move_end(&self, end: f64) -> f64 {
        const STEP: f64 = 1.0;
        let window = self.config.edge_window;
        let limit = end + self.config.window;
        // A match can run past the true edge, so the walk starts well inside the region.
        let mut time = end - self.config.window - window;
        if !self.agrees_at(time) {
            return end;
        }
        while time + window + STEP <= limit && self.agrees_at(time + STEP) {
            time += STEP;
        }
        time + window
    }

    fn move_start(&self, start: f64) -> f64 {
        const STEP: f64 = 1.0;
        let limit = start - self.config.window;
        let mut time = start + self.config.window;
        if !self.agrees_at(time) {
            return start;
        }
        while time - STEP >= limit && self.agrees_at(time - STEP) {
            time -= STEP;
        }
        time
    }

    /// Silence says nothing, so it lets the edge pass, up to the limit.
    fn agrees_at(&self, time: f64) -> bool {
        let length = self.current.samples(self.config.edge_window);
        let (Some(a), Some(b)) = (
            self.current.index_of(time),
            self.earlier.index_of(time - self.offset),
        ) else {
            return false;
        };
        if a + length > self.current.len() || b + length > self.earlier.len() {
            return false;
        }
        let (mean_a, deviation_a) = self.current.stats(a, length);
        let (mean_b, deviation_b) = self.earlier.stats(b, length);
        if deviation_a < MIN_REFERENCE_DEVIATION || deviation_b < MIN_REFERENCE_DEVIATION {
            return true;
        }
        let product: f64 = self.current.filled[a..a + length]
            .iter()
            .zip(&self.earlier.filled[b..b + length])
            .map(|(x, y)| (x - mean_a) * (y - mean_b))
            .sum();
        product / (length as f64 * deviation_a * deviation_b) >= self.config.edge_threshold
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

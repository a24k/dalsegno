//! Moving region edges to about a second, at a fixed shift.

use super::super::same_audio::{compare_audio, is_same_audio};
use super::super::series::Prepared;
use super::{DiffConfig, Region, Span, VersionCycle, Window};

/// Moves the edges of matched regions from window precision to about a second.
///
/// A window straddling an edge matches nothing, so raw regions stop short of the truth.
pub(super) fn refine_edges(
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
                    period: versions[found.version].period,
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

pub(super) struct EdgeComparison<'a> {
    pub(super) current: &'a Prepared,
    pub(super) earlier: &'a Prepared,
    pub(super) period: f64,
    pub(super) offset: f64,
    pub(super) config: &'a DiffConfig,
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
        let seconds = self.config.edge_window;
        let earlier_time = self.earlier_time(time, seconds);
        compare_audio(self.current, time, self.earlier, earlier_time, seconds)
            .is_some_and(|(score, _)| score >= self.config.edge_threshold)
    }

    /// Returns whether `seconds` of audio from `time` match at this offset. Stretches that
    /// cannot be compared do not.
    pub(super) fn is_same(&self, time: f64, seconds: f64) -> bool {
        let earlier_time = self.earlier_time(time, seconds);
        is_same_audio(self.current, time, self.earlier, earlier_time, seconds).unwrap_or(false)
    }

    /// Returns where `time` falls in the earlier series. It loops, so a position past the
    /// stored audio is found one period back.
    pub(super) fn earlier_time(&self, time: f64, seconds: f64) -> f64 {
        let position = time - self.offset;
        let overrun = position + seconds - self.earlier.end_time();
        if overrun > 0.0 {
            position - (overrun / self.period).ceil() * self.period
        } else {
            position
        }
    }
}

//! Giving back the videos of a new region that are the same audio.
//!
//! A stretch is judged as a whole, so one replaced video makes all of it new. Judged video by
//! video, the rest of the stretch matches, and what stays new has the edges of the real change.

use super::super::series::Prepared;
use super::super::videos::{nearest_silence, split_at_silences};
use super::edges::EdgeComparison;
use super::{
    DiffConfig, Found, Region, RegionClass, Span, VersionCycle, Window, circular_distance, same_key,
};
use std::iter::successors;

/// How far an edge of new audio moves to the silence between two videos, in seconds. Edges land
/// within about half a window of the truth.
const EDGE_REACH: f64 = 40.0;
/// Audio compared at a time when walking an edge into new audio, in seconds. Too short to hide a
/// change, and long enough that the level alone tells unrelated audio apart.
const WALK_SECONDS: f64 = 20.0;
/// How far an edge moves at a time, in seconds.
const WALK_STEP: f64 = 5.0;

/// A matched neighbour of a new region.
#[derive(Clone, Copy)]
struct Neighbour<'a> {
    region: Region,
    found: Found,
    comparison: &'a EdgeComparison<'a>,
}

impl Neighbour<'_> {
    fn holds(&self, (start, end): (f64, f64)) -> bool {
        self.comparison.is_same(start, end - start)
    }

    fn claim(&self, (start, end): (f64, f64)) -> Region {
        Region {
            start,
            end,
            match_start: Some(self.comparison.earlier_time(start, end - start)),
            ..self.region
        }
    }

    /// Walks forward from the start of new audio for as long as it matches here. Returns the
    /// start of what stays new.
    fn walk_forward(&self, (start, end): (f64, f64)) -> f64 {
        let fits = |time: f64| time + WALK_SECONDS <= end;
        let holds = |time: f64| fits(time) && self.holds((time, time + WALK_SECONDS));
        let matched = successors(Some(start), |time| Some(time + WALK_STEP))
            .take_while(|time| holds(*time))
            .last();
        matched.map_or(start, |time| time + WALK_SECONDS)
    }

    /// Walks back from the end of new audio for as long as it matches here. Returns the end of
    /// what stays new.
    fn walk_back(&self, (start, end): (f64, f64)) -> f64 {
        let fits = |time: f64| time - WALK_SECONDS >= start;
        let holds = |time: f64| fits(time) && self.holds((time - WALK_SECONDS, time));
        let matched = successors(Some(end), |time| Some(time - WALK_STEP))
            .take_while(|time| holds(*time))
            .last();
        matched.map_or(end, |time| time - WALK_SECONDS)
    }
}

/// Splits new regions into videos, and gives each one that matches to the neighbour it matches.
pub(super) fn reclaim_videos<'a>(
    regions: &[Region],
    spans: &[Span],
    current: &'a Prepared,
    versions: &[VersionCycle<'a>],
    config: &'a DiffConfig,
) -> Vec<Region> {
    let comparisons: Vec<Option<EdgeComparison<'a>>> = spans
        .iter()
        .map(|span| match span.window {
            Window::Found(found) => Some(EdgeComparison::at(found, current, versions, config)),
            Window::NoData | Window::NoMatch => None,
        })
        .collect();
    let neighbour = |index: usize| {
        let Window::Found(found) = spans.get(index)?.window else {
            return None;
        };
        Some(Neighbour {
            region: regions[index],
            found,
            comparison: comparisons[index].as_ref()?,
        })
    };
    let mut result = Vec::with_capacity(regions.len());
    for (index, region) in regions.iter().enumerate() {
        if region.class != RegionClass::New {
            result.push(*region);
            continue;
        }
        let before = index.checked_sub(1).and_then(neighbour);
        let after = neighbour(index + 1);
        let start = before.map_or(region.start, |side| side.region.end);
        let end = after.map_or(region.end, |side| side.region.start);
        let pieces = split_at_silences(current, start, end);
        let is_replacement = before
            .zip(after)
            .is_some_and(|(a, b)| same_key(a.found, b.found, versions, config));
        let claims = if is_replacement {
            claim_each(&pieces, before, after)
        } else {
            claim_from_ends(&pieces, before, after)
        };
        // Nothing located the difference, so the whole stretch stands as it was judged.
        if claims.iter().all(Option::is_some) {
            result.push(*region);
            continue;
        }
        let parts: Vec<Region> = claims
            .iter()
            .zip(&pieces)
            .map(|(claim, piece)| {
                claim.unwrap_or(Region {
                    start: piece.0,
                    end: piece.1,
                    ..*region
                })
            })
            .collect();
        result.extend(if is_replacement {
            parts
        } else {
            walk_edges(parts, before, after)
        });
    }
    snap_edges(&result, current, config)
}

/// Gives the neighbours what still matches at the ends of the new audio. Between two videos with
/// no silence, nothing has split the stretch, and flat audio correlates too poorly to place an
/// edge, so it is compared in short blocks at the neighbour's shift.
fn walk_edges(
    parts: Vec<Region>,
    before: Option<Neighbour<'_>>,
    after: Option<Neighbour<'_>>,
) -> Vec<Region> {
    let is_new = |part: &Region| part.class == RegionClass::New;
    let (Some(first), Some(last)) = (
        parts.iter().position(is_new),
        parts.iter().rposition(is_new),
    ) else {
        return parts;
    };
    let mut walked = parts;
    if let Some(side) = after {
        let span = (walked[last].start, walked[last].end);
        let end = side.walk_back(span);
        if end < span.1 {
            walked[last].end = end;
            walked.insert(last + 1, side.claim((end, span.1)));
        }
    }
    if let Some(side) = before {
        let span = (walked[first].start, walked[first].end);
        let start = side.walk_forward(span);
        if start > span.0 {
            walked[first].start = start;
            walked.insert(first, side.claim((span.0, start)));
        }
    }
    walked
}

/// Moves each edge between new and matched audio to a silence nearby, as videos change there.
fn snap_edges(regions: &[Region], current: &Prepared, config: &DiffConfig) -> Vec<Region> {
    let is_new = |region: &Region| region.class == RegionClass::New;
    let is_matched = |region: &Region| region.shift.is_some();
    let mut snapped = regions.to_vec();
    for index in 1..snapped.len() {
        let (before, after) = (snapped[index - 1], snapped[index]);
        let is_edge =
            (is_new(&before) && is_matched(&after)) || (is_matched(&before) && is_new(&after));
        if !is_edge {
            continue;
        }
        let keeps_both = |time: &f64| {
            time - before.start >= config.min_region_seconds
                && after.end - time >= config.min_region_seconds
        };
        if let Some(time) = nearest_silence(current, after.start, EDGE_REACH).filter(keeps_both) {
            snapped[index - 1].end = time;
            snapped[index].start = time;
        }
    }
    snapped
}

/// Audio between two equal shifts plays in order, so each video can match on its own. Equal
/// shifts can differ by the shift tolerance, which is far more than a video can bear, so a video
/// may match either side.
fn claim_each(
    pieces: &[(f64, f64)],
    before: Option<Neighbour<'_>>,
    after: Option<Neighbour<'_>>,
) -> Vec<Option<Region>> {
    pieces
        .iter()
        .map(|piece| {
            let holds = |side: &Neighbour<'_>| side.holds(*piece);
            before
                .filter(holds)
                .or(after.filter(holds))
                .map(|side| side.claim(*piece))
        })
        .collect()
}

/// Audio between two different shifts holds a change, so matches run in from each end.
fn claim_from_ends(
    pieces: &[(f64, f64)],
    before: Option<Neighbour<'_>>,
    after: Option<Neighbour<'_>>,
) -> Vec<Option<Region>> {
    let from_start = before.map_or(0, |side| {
        pieces
            .iter()
            .take_while(|piece| side.holds(**piece))
            .count()
    });
    let rest = &pieces[from_start..];
    let from_end = after.map_or(0, |side| {
        rest.iter()
            .rev()
            .take_while(|piece| side.holds(**piece))
            .count()
    });
    let claim_at = |index: usize| {
        let piece = pieces[index];
        if index < from_start {
            before.map(|side| side.claim(piece))
        } else if index >= pieces.len() - from_end {
            after.map(|side| side.claim(piece))
        } else {
            None
        }
    };
    (0..pieces.len()).map(claim_at).collect()
}

/// Joins neighbouring regions that are one stretch: new audio, or one shift of one version.
/// Known clips stay apart, as each one is a clip.
pub(super) fn join_regions(
    regions: &[Region],
    versions: &[VersionCycle<'_>],
    config: &DiffConfig,
) -> Vec<Region> {
    let joins = |a: &Region, b: &Region| {
        let cycle = versions.iter().find(|cycle| Some(cycle.id) == a.version);
        let same_shift = match (a.shift, b.shift, cycle) {
            (None, None, _) => a.class != RegionClass::Known,
            (Some(x), Some(y), Some(cycle)) => {
                circular_distance(x, y, cycle.period) <= config.shift_tolerance
            }
            _ => false,
        };
        a.class == b.class && a.version == b.version && same_shift
    };
    let mut joined: Vec<Region> = Vec::with_capacity(regions.len());
    for region in regions {
        match joined.last_mut() {
            Some(last) if joins(last, region) => last.end = region.end,
            _ => joined.push(*region),
        }
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::super::super::synthetic::{content, prepared};
    use super::*;

    fn region(class: RegionClass, start: f64, end: f64) -> Region {
        Region {
            class,
            start,
            end,
            shift: (class == RegionClass::Existing).then_some(5.0),
            version: (class == RegionClass::Existing).then_some(1),
            match_start: None,
            moved: false,
        }
    }

    fn with_silence_at(seconds: usize) -> Prepared {
        let values = [
            content(1, seconds as f64 - 1.0),
            vec![0.0; 20],
            content(2, 300.0),
        ]
        .concat();
        prepared(&values)
    }

    #[test]
    fn an_edge_moves_to_a_silence_nearby() {
        let regions = [
            region(RegionClass::Existing, 0.0, 110.0),
            region(RegionClass::New, 110.0, 200.0),
        ];

        let snapped = snap_edges(&regions, &with_silence_at(100), &DiffConfig::default());

        assert_eq!(snapped[0].end, 100.0);
        assert_eq!(snapped[1].start, 100.0);
    }

    #[test]
    fn an_edge_does_not_pass_through_a_short_matched_region() {
        let regions = [
            region(RegionClass::Existing, 105.0, 125.0),
            region(RegionClass::New, 125.0, 200.0),
        ];

        let snapped = snap_edges(&regions, &with_silence_at(100), &DiffConfig::default());

        assert_eq!(snapped[..], regions);
    }

    #[test]
    fn an_edge_does_not_swallow_a_short_new_region() {
        let regions = [
            region(RegionClass::Existing, 0.0, 90.0),
            region(RegionClass::New, 90.0, 105.0),
            region(RegionClass::Existing, 105.0, 200.0),
        ];

        let snapped = snap_edges(&regions, &with_silence_at(100), &DiffConfig::default());

        assert_eq!(snapped[..], regions);
    }

    #[test]
    fn known_clips_stay_apart() {
        let regions = [
            region(RegionClass::Known, 0.0, 100.0),
            region(RegionClass::Known, 100.0, 200.0),
        ];

        assert_eq!(join_regions(&regions, &[], &DiffConfig::default()).len(), 2);
    }

    #[test]
    fn new_audio_joins() {
        let regions = [
            region(RegionClass::New, 0.0, 100.0),
            region(RegionClass::New, 100.0, 200.0),
        ];

        assert_eq!(join_regions(&regions, &[], &DiffConfig::default()).len(), 1);
    }
}

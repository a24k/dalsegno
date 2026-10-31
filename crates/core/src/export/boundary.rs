//! Boundary candidates from black and silence detection (FR-E2, FR-E3, FR-E4).
//!
//! The tool proposes and a person decides (G3), so candidates are ranked and none is chosen.

use serde::Serialize;

/// How far before a black or silent stretch a keyframe still counts for it, in seconds.
const KEYFRAME_LEAD_SECONDS: f64 = 5.0;
/// Black in two cycles counts as the same when the differences agree to this, in seconds.
const PAIR_TOLERANCE_SECONDS: f64 = 0.05;
/// How far from the expected period a pair of blacks may lie, in seconds.
const PERIOD_SEARCH_SECONDS: f64 = 5.0;

/// A stretch of a detector's output.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Interval {
    /// Start, in seconds.
    pub start: f64,
    /// End, in seconds.
    pub end: f64,
}

impl Interval {
    fn overlap(&self, other: &Self) -> Option<Self> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        (start < end).then_some(Self { start, end })
    }
}

/// Reads ffmpeg's `blackdetect` log.
pub fn parse_black(log: &str) -> Vec<Interval> {
    log.lines()
        .filter_map(|line| {
            let start = number_after(line, "black_start:")?;
            let end = number_after(line, "black_end:")?;
            Some(Interval { start, end })
        })
        .collect()
}

/// Reads ffmpeg's `silencedetect` log. Silence still running at `end_of_file` ends there.
pub fn parse_silence(log: &str, end_of_file: f64) -> Vec<Interval> {
    let mut intervals = Vec::new();
    let mut open = None;
    for line in log.lines() {
        if let Some(start) = number_after(line, "silence_start:") {
            open = Some(start);
        } else if let (Some(start), Some(end)) = (open, number_after(line, "silence_end:")) {
            intervals.push(Interval { start, end });
            open = None;
        }
    }
    if let Some(start) = open {
        intervals.push(Interval {
            start,
            end: end_of_file,
        });
    }
    intervals
}

fn number_after(line: &str, key: &str) -> Option<f64> {
    let (_, rest) = line.split_once(key)?;
    let value = rest.split_whitespace().next()?;
    value.parse().ok().filter(|number: &f64| number.is_finite())
}

/// What a candidate rests on. Black or silence alone is often part of a song.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// Black and silence overlap.
    Both,
    /// Only black.
    BlackOnly,
    /// Only silence.
    SilenceOnly,
}

/// A fragment start, which is a keyframe (D-06).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Keyframe {
    /// Fragment number.
    pub sq: u64,
    /// Stream time.
    pub time: f64,
}

/// A place a cut can start.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct StartCandidate {
    /// The keyframe to start from.
    pub keyframe: Keyframe,
    /// What it rests on.
    pub evidence: Evidence,
    /// The stretch it belongs to.
    pub interval: Interval,
}

/// A place a cut can end, which may be any frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct EndCandidate {
    /// Stream time, the moment the sound disappears.
    pub time: f64,
    /// What it rests on.
    pub evidence: Evidence,
}

/// Sorts stretches by evidence, then by time, with overlaps first.
fn ranked(black: &[Interval], silence: &[Interval]) -> Vec<(Interval, Evidence)> {
    let overlaps: Vec<Interval> = black
        .iter()
        .flat_map(|b| silence.iter().filter_map(|s| b.overlap(s)))
        .collect();
    let is_free = |interval: &Interval| !overlaps.iter().any(|o| o.overlap(interval).is_some());
    let mut all: Vec<(Interval, Evidence)> = overlaps
        .iter()
        .map(|interval| (*interval, Evidence::Both))
        .chain(
            black
                .iter()
                .filter(|interval| is_free(interval))
                .map(|interval| (*interval, Evidence::BlackOnly)),
        )
        .chain(
            silence
                .iter()
                .filter(|interval| is_free(interval))
                .map(|interval| (*interval, Evidence::SilenceOnly)),
        )
        .collect();
    all.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.start.total_cmp(&b.0.start)));
    all
}

/// Offers the keyframes inside each stretch, or the last one just before it.
pub fn start_candidates(
    black: &[Interval],
    silence: &[Interval],
    keyframes: &[Keyframe],
) -> Vec<StartCandidate> {
    ranked(black, silence)
        .into_iter()
        .flat_map(|(interval, evidence)| {
            let inside: Vec<Keyframe> = keyframes
                .iter()
                .filter(|frame| (interval.start..=interval.end).contains(&frame.time))
                .copied()
                .collect();
            let chosen = if inside.is_empty() {
                keyframes
                    .iter()
                    .rev()
                    .find(|frame| {
                        frame.time < interval.start
                            && interval.start - frame.time <= KEYFRAME_LEAD_SECONDS
                    })
                    .copied()
                    .into_iter()
                    .collect()
            } else {
                inside
            };
            chosen.into_iter().map(move |keyframe| StartCandidate {
                keyframe,
                evidence,
                interval,
            })
        })
        .collect()
}

/// Offers the moment the sound stops, so that not one frame of what follows gets in.
pub fn end_candidates(black: &[Interval], silence: &[Interval]) -> Vec<EndCandidate> {
    let mut candidates: Vec<EndCandidate> = silence
        .iter()
        .map(|interval| EndCandidate {
            time: interval.start,
            evidence: if black.iter().any(|b| b.overlap(interval).is_some()) {
                Evidence::Both
            } else {
                Evidence::SilenceOnly
            },
        })
        .collect();
    candidates.sort_by(|a, b| a.evidence.cmp(&b.evidence).then(a.time.total_cmp(&b.time)));
    candidates
}

/// The period from black at a boundary and one cycle later.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct RefinedPeriod {
    /// Period, in seconds.
    pub period: f64,
    /// How many pairs of edges agree. One stretch of black gives two.
    pub agreeing: usize,
}

/// Finds the difference that most edges of black agree on, near `expected`.
pub fn refine_period(
    first: &[Interval],
    later: &[Interval],
    expected: f64,
) -> Option<RefinedPeriod> {
    let edges = |intervals: &[Interval]| -> Vec<f64> {
        intervals.iter().flat_map(|i| [i.start, i.end]).collect()
    };
    let (from, to) = (edges(first), edges(later));
    let differences: Vec<f64> = from
        .iter()
        .flat_map(|a| to.iter().map(move |b| b - a))
        .filter(|difference| (difference - expected).abs() <= PERIOD_SEARCH_SECONDS)
        .collect();
    let agreeing_with = |value: f64| -> Vec<f64> {
        differences
            .iter()
            .copied()
            .filter(|other| (other - value).abs() <= PAIR_TOLERANCE_SECONDS)
            .collect()
    };
    let best = differences
        .iter()
        .map(|value| agreeing_with(*value))
        .max_by_key(Vec::len)?;
    Some(RefinedPeriod {
        period: best.iter().sum::<f64>() / best.len() as f64,
        agreeing: best.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn interval(start: f64, end: f64) -> Interval {
        Interval { start, end }
    }

    fn keyframes() -> Vec<Keyframe> {
        (0..10)
            .map(|index| Keyframe {
                sq: 100 + index,
                time: 5.0 * index as f64,
            })
            .collect()
    }

    #[test]
    fn black_lines_are_read() {
        let log = "[blackdetect @ 0x1] black_start:166.633 black_end:167.2 black_duration:0.567\n\
                   frame= 12 fps=0\n";

        assert_eq!(parse_black(log), [interval(166.633, 167.2)]);
    }

    #[test]
    fn silence_lines_are_read() {
        let log = "[silencedetect @ 0x1] silence_start: 200.72\n\
                   [silencedetect @ 0x1] silence_end: 201.76 | silence_duration: 1.04\n";

        assert_eq!(parse_silence(log, 300.0), [interval(200.72, 201.76)]);
    }

    #[test]
    fn silence_running_to_the_end_ends_there() {
        let log = "[silencedetect @ 0x1] silence_start: 290\n";

        assert_eq!(parse_silence(log, 300.0), [interval(290.0, 300.0)]);
    }

    #[test]
    fn nonsense_is_ignored() {
        assert_eq!(parse_black("black_start:nan black_end:1"), []);
        assert_eq!(parse_black("black_start:1"), []);
        assert_eq!(parse_silence("silence_end: 4", 9.0), []);
    }

    #[test]
    fn overlaps_rank_above_either_alone() {
        let black = [interval(20.0, 21.0), interval(40.0, 41.0)];
        let silence = [interval(20.5, 22.0), interval(30.0, 31.0)];

        let candidates = start_candidates(&black, &silence, &keyframes());

        let evidence: Vec<Evidence> = candidates.iter().map(|c| c.evidence).collect();
        assert_eq!(
            evidence,
            [Evidence::Both, Evidence::BlackOnly, Evidence::SilenceOnly]
        );
    }

    #[test]
    fn a_keyframe_inside_a_stretch_is_offered() {
        let candidates = start_candidates(&[interval(14.0, 16.0)], &[], &keyframes());

        assert_eq!(candidates[0].keyframe.sq, 103);
    }

    #[test]
    fn every_keyframe_inside_is_offered_for_a_person_to_choose() {
        let candidates = start_candidates(&[interval(9.0, 21.0)], &[], &keyframes());

        let frames: Vec<u64> = candidates.iter().map(|c| c.keyframe.sq).collect();
        assert_eq!(frames, [102, 103, 104]);
    }

    #[test]
    fn the_keyframe_just_before_is_offered_when_none_is_inside() {
        let candidates = start_candidates(&[interval(16.0, 17.0)], &[], &keyframes());

        assert_eq!(candidates[0].keyframe.sq, 103);
    }

    #[test]
    fn a_keyframe_far_before_is_not_offered() {
        let far = [Keyframe { sq: 1, time: 0.0 }];

        assert_eq!(start_candidates(&[interval(16.0, 17.0)], &[], &far), []);
    }

    #[test]
    fn ends_are_the_moments_the_sound_stops() {
        let silence = [interval(10.0, 11.0), interval(30.0, 31.0)];
        let black = [interval(30.5, 32.0)];

        let ends = end_candidates(&black, &silence);

        assert_eq!(
            ends,
            [
                EndCandidate {
                    time: 30.0,
                    evidence: Evidence::Both
                },
                EndCandidate {
                    time: 10.0,
                    evidence: Evidence::SilenceOnly
                },
            ]
        );
    }

    // The worked example from the measured facts: two blacks, 37,649.067 s apart.
    #[test]
    fn the_period_comes_from_equal_differences() {
        let first = [interval(166.633, 170.0), interval(173.866, 175.0)];
        let later = [
            interval(37_815.700, 37_819.067),
            interval(37_822.933, 37_824.067),
        ];

        let refined = refine_period(&first, &later, 37_648.7).unwrap();

        assert!((refined.period - 37_649.067).abs() < 0.001, "{refined:?}");
        assert_eq!(refined.agreeing, 4);
    }

    #[test]
    fn black_nowhere_near_the_period_gives_nothing() {
        let found = refine_period(&[interval(0.0, 1.0)], &[interval(500.0, 501.0)], 100.0);

        assert_eq!(found, None);
    }

    #[test]
    fn no_black_gives_nothing() {
        assert_eq!(refine_period(&[], &[], 100.0), None);
    }
}

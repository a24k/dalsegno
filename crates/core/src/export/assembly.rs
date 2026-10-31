//! Which fragments a cut needs (FR-E4, FR-E5, D-06).

use serde::Serialize;
use thiserror::Error;

/// A fragment of the track being cut.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Piece {
    /// Fragment number.
    pub sq: u64,
    /// Stream time of its start.
    pub time: f64,
    /// Its length.
    pub duration: f64,
}

/// The range to concatenate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FragmentRange {
    /// First fragment, whose start is the chosen keyframe.
    pub sq_first: u64,
    /// Fragment containing the end of the cut.
    pub sq_last: u64,
}

/// Why a cut cannot go ahead.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum CutError {
    /// The cut has no length.
    #[error("a cut must last longer than zero seconds, not {0}")]
    NotPositive(f64),
    /// No fragment starts at the chosen keyframe.
    #[error("no fragment starts at {0}")]
    NoKeyframe(f64),
    /// The fragments stop before the cut does.
    #[error("the cut runs to {needed}, and the fragments stop at {available}")]
    ShortOfFragments {
        /// End of the cut.
        needed: f64,
        /// End of the last fragment.
        available: f64,
    },
    /// Fragments are missing in the middle of the cut.
    #[error("fragment {0} is missing from the cut")]
    Hole(u64),
    /// The cut reaches outside the time its version was on air.
    #[error("the cut {start}..{end} is outside the version's air time {from}..{until:?}")]
    OutsideAirTime {
        /// Start of the cut.
        start: f64,
        /// End of the cut.
        end: f64,
        /// When the version began.
        from: f64,
        /// When it was superseded, if it was.
        until: Option<f64>,
    },
}

/// How far a keyframe time may be from a fragment start, in seconds.
const KEYFRAME_TOLERANCE_SECONDS: f64 = 0.001;
/// Fragment lengths are measured, so an end this close to a fragment's end needs no more.
const END_TOLERANCE_SECONDS: f64 = 0.001;

/// Finds the fragments of a cut, which must be sorted and with no gaps inside it.
pub fn plan_cut(pieces: &[Piece], start: f64, duration: f64) -> Result<FragmentRange, CutError> {
    if duration <= 0.0 {
        return Err(CutError::NotPositive(duration));
    }
    let first = pieces
        .iter()
        .position(|piece| (piece.time - start).abs() <= KEYFRAME_TOLERANCE_SECONDS)
        .ok_or(CutError::NoKeyframe(start))?;
    let end = start + duration;
    let reaches_end = |piece: &Piece| piece.time + piece.duration >= end - END_TOLERANCE_SECONDS;
    let mut last = pieces[first];
    for piece in &pieces[first + 1..] {
        if reaches_end(&last) {
            break;
        }
        if piece.sq != last.sq + 1 {
            return Err(CutError::Hole(last.sq + 1));
        }
        last = *piece;
    }
    if !reaches_end(&last) {
        return Err(CutError::ShortOfFragments {
            needed: end,
            available: last.time + last.duration,
        });
    }
    Ok(FragmentRange {
        sq_first: pieces[first].sq,
        sq_last: last.sq,
    })
}

/// Checks that a full-cycle cut lies within the time its version was on air (FR-E5).
pub fn check_air_time(start: f64, end: f64, from: f64, until: Option<f64>) -> Result<(), CutError> {
    let is_inside = start >= from && until.is_none_or(|limit| end <= limit);
    if is_inside {
        Ok(())
    } else {
        Err(CutError::OutsideAirTime {
            start,
            end,
            from,
            until,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pieces(sqs: std::ops::Range<u64>) -> Vec<Piece> {
        sqs.map(|sq| Piece {
            sq,
            time: sq as f64 * 5.0,
            duration: 5.0,
        })
        .collect()
    }

    #[test]
    fn a_cut_runs_from_its_keyframe_to_the_fragment_holding_its_end() {
        let range = plan_cut(&pieces(10..30), 60.0, 12.0).unwrap();

        assert_eq!(
            range,
            FragmentRange {
                sq_first: 12,
                sq_last: 14
            }
        );
    }

    #[test]
    fn an_end_on_a_fragment_boundary_needs_no_more() {
        let range = plan_cut(&pieces(10..30), 60.0, 10.0).unwrap();

        assert_eq!(range.sq_last, 13);
    }

    #[test]
    fn a_start_that_is_no_keyframe_is_rejected() {
        assert_eq!(
            plan_cut(&pieces(10..30), 61.0, 5.0),
            Err(CutError::NoKeyframe(61.0))
        );
    }

    #[test]
    fn a_cut_past_the_fragments_is_rejected() {
        assert!(matches!(
            plan_cut(&pieces(10..13), 50.0, 100.0),
            Err(CutError::ShortOfFragments { .. })
        ));
    }

    #[test]
    fn a_hole_inside_a_cut_is_rejected() {
        let list: Vec<Piece> = pieces(10..30)
            .into_iter()
            .filter(|piece| piece.sq != 15)
            .collect();

        assert_eq!(plan_cut(&list, 50.0, 60.0), Err(CutError::Hole(15)));
    }

    #[test]
    fn a_hole_past_the_end_of_a_cut_is_no_problem() {
        let list: Vec<Piece> = pieces(10..30)
            .into_iter()
            .filter(|piece| piece.sq != 20)
            .collect();

        assert!(plan_cut(&list, 50.0, 10.0).is_ok());
    }

    #[test]
    fn a_fragment_a_hair_short_of_the_end_is_enough() {
        let range = plan_cut(&pieces(10..13), 50.0, 15.0005).unwrap();

        assert_eq!(range.sq_last, 12);
    }

    #[test]
    fn a_cut_of_no_length_is_rejected() {
        assert_eq!(
            plan_cut(&pieces(10..30), 50.0, 0.0),
            Err(CutError::NotPositive(0.0))
        );
    }

    #[test]
    fn a_cut_inside_the_air_time_passes() {
        assert_eq!(check_air_time(10.0, 20.0, 5.0, Some(30.0)), Ok(()));
        assert_eq!(check_air_time(10.0, 20.0, 5.0, None), Ok(()));
    }

    #[test]
    fn a_cut_outside_the_air_time_is_blocked() {
        assert!(check_air_time(1.0, 20.0, 5.0, Some(30.0)).is_err());
        assert!(check_air_time(10.0, 40.0, 5.0, Some(30.0)).is_err());
    }
}

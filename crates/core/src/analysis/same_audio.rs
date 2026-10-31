//! Asking whether two stretches at a known distance are the same audio.
//!
//! This is one comparison and not a search over a cycle, so far weaker evidence is enough, and
//! it decides what chance matches in a search cannot. Flat, loud music correlates poorly with
//! itself, while its level stays close.

use super::ncc::MIN_REFERENCE_DEVIATION;
use super::series::Prepared;

/// Correlation that counts as the same audio.
pub(super) const MIN_CORRELATION: f64 = 0.3;
/// Mean absolute difference above which audio is not the same.
pub(super) const MAX_DIFFERENCE: f64 = 0.35;

/// Bins of two versions sit up to a bin apart, which inflates the difference wherever the level
/// moves fast, so the nearest offsets are tried too.
const NUDGES: [f64; 5] = [0.0, -0.1, 0.1, -0.2, 0.2];

/// Returns the correlation and mean absolute difference of `seconds` of audio.
///
/// Stretches too flat to correlate score 1, as they are no evidence against.
pub(super) fn compare_audio(
    current: &Prepared,
    time: f64,
    earlier: &Prepared,
    earlier_time: f64,
    seconds: f64,
) -> Option<(f64, f64)> {
    let length = current.samples(seconds);
    let a = current.index_of(time)?;
    let b = earlier.index_of(earlier_time)?;
    if length == 0 || a + length > current.len() || b + length > earlier.len() {
        return None;
    }
    let (mean_a, deviation_a) = current.stats(a, length);
    let (mean_b, deviation_b) = earlier.stats(b, length);
    let pairs = current.filled[a..a + length]
        .iter()
        .zip(&earlier.filled[b..b + length]);
    let difference = pairs.clone().map(|(x, y)| (x - y).abs()).sum::<f64>() / length as f64;
    if deviation_a < MIN_REFERENCE_DEVIATION || deviation_b < MIN_REFERENCE_DEVIATION {
        return Some((1.0, difference));
    }
    let product: f64 = pairs.map(|(x, y)| (x - mean_a) * (y - mean_b)).sum();
    Some((
        product / (length as f64 * deviation_a * deviation_b),
        difference,
    ))
}

/// Returns whether the stretches are the same audio, or `None` when they cannot be compared.
pub(super) fn is_same_audio(
    current: &Prepared,
    time: f64,
    earlier: &Prepared,
    earlier_time: f64,
    seconds: f64,
) -> Option<bool> {
    let results: Vec<(f64, f64)> = NUDGES
        .iter()
        .filter_map(|nudge| compare_audio(current, time, earlier, earlier_time + nudge, seconds))
        .collect();
    if results.is_empty() {
        return None;
    }
    Some(
        results
            .iter()
            .any(|(score, difference)| *score >= MIN_CORRELATION && *difference <= MAX_DIFFERENCE),
    )
}

#[cfg(test)]
mod tests {
    use super::super::synthetic::{content, prepared};
    use super::*;

    #[test]
    fn a_stretch_is_the_same_as_itself() {
        let data = prepared(&content(1, 300.0));

        assert_eq!(is_same_audio(&data, 50.0, &data, 50.0, 60.0), Some(true));
    }

    #[test]
    fn unrelated_audio_is_not_the_same() {
        let data = prepared(&[content(2, 100.0), content(3, 100.0)].concat());

        assert_eq!(is_same_audio(&data, 110.0, &data, 10.0, 60.0), Some(false));
    }

    #[test]
    fn a_small_time_offset_is_forgiven() {
        let values = content(4, 300.0);
        let later = prepared(&values[2..]);
        let earlier = prepared(&values);

        assert_eq!(
            is_same_audio(&later, 50.0, &earlier, 50.0, 60.0),
            Some(true)
        );
    }

    #[test]
    fn a_level_far_away_is_not_the_same_even_when_flat() {
        let data = prepared(&[vec![5.0; 1000], vec![9.0; 1000]].concat());

        assert_eq!(is_same_audio(&data, 110.0, &data, 10.0, 60.0), Some(false));
    }

    #[test]
    fn stretches_outside_the_series_cannot_be_compared() {
        let data = prepared(&content(5, 100.0));

        assert_eq!(is_same_audio(&data, 80.0, &data, 10.0, 60.0), None);
        assert_eq!(is_same_audio(&data, 10.0, &data, -50.0, 60.0), None);
        assert_eq!(is_same_audio(&data, 10.0, &data, 10.0, 0.0), None);
    }
}

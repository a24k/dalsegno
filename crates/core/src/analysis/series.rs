//! A feature over time, with its holes explicit.

/// A window with a larger share of invalid samples scores nothing.
const MAX_INVALID_SHARE: f64 = 0.1;

/// Values at a constant rate, some of which may be missing.
#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    /// Stream time of the first value, in seconds.
    pub start_time: f64,
    /// Values per second.
    pub rate: f64,
    /// The values.
    pub values: Vec<f32>,
    /// Whether each value was measured.
    pub valid: Vec<bool>,
}

impl Series {
    /// Wraps values with no holes.
    pub fn complete(start_time: f64, rate: f64, values: Vec<f32>) -> Self {
        let valid = vec![true; values.len()];
        Self {
            start_time,
            rate,
            values,
            valid,
        }
    }
}

/// A series made ready for correlation: holes filled, and cumulative sums taken in `f64`,
/// because `f32` loses too much over millions of samples.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub(crate) start_time: f64,
    pub(crate) rate: f64,
    pub(crate) filled: Vec<f64>,
    sums: Vec<f64>,
    squares: Vec<f64>,
    invalid: Vec<u32>,
}

impl Prepared {
    /// Fills holes with the mean of the values on either side, and takes the sums.
    pub fn new(series: &Series) -> Self {
        let measured: Vec<bool> = series
            .values
            .iter()
            .zip(&series.valid)
            .map(|(value, valid)| *valid && value.is_finite())
            .collect();
        let filled = fill_holes(&series.values, &measured);
        let sums = cumulative(filled.iter().copied());
        let squares = cumulative(filled.iter().map(|value| value * value));
        let invalid = std::iter::once(0)
            .chain(measured.iter().scan(0, |count, valid| {
                *count += u32::from(!valid);
                Some(*count)
            }))
            .collect();
        Self {
            start_time: series.start_time,
            rate: series.rate,
            filled,
            sums,
            squares,
            invalid,
        }
    }

    /// Returns the number of samples.
    pub fn len(&self) -> usize {
        self.filled.len()
    }

    /// Returns whether there are no samples.
    pub fn is_empty(&self) -> bool {
        self.filled.is_empty()
    }

    /// Returns the stream time just past the last sample.
    pub fn end_time(&self) -> f64 {
        self.time_of(self.len())
    }

    /// Returns the stream time of a sample.
    pub fn time_of(&self, index: usize) -> f64 {
        self.start_time + index as f64 / self.rate
    }

    /// Returns the sample at a stream time, or `None` before the series.
    pub fn index_of(&self, time: f64) -> Option<usize> {
        let index = ((time - self.start_time) * self.rate).round();
        (index >= 0.0).then_some(index as usize)
    }

    /// Converts seconds to samples.
    pub fn samples(&self, seconds: f64) -> usize {
        (seconds * self.rate).round() as usize
    }

    /// Returns the mean and standard deviation of a window.
    pub(crate) fn stats(&self, start: usize, length: usize) -> (f64, f64) {
        let end = start + length;
        let count = length as f64;
        let mean = (self.sums[end] - self.sums[start]) / count;
        let energy = (self.squares[end] - self.squares[start]) / count;
        (mean, (energy - mean * mean).max(0.0).sqrt())
    }

    /// Returns whether a window has enough measured samples to score.
    pub(crate) fn has_data(&self, start: usize, length: usize) -> bool {
        let Some(end_count) = self.invalid.get(start + length) else {
            return false;
        };
        let missing = f64::from(end_count - self.invalid[start]);
        missing <= MAX_INVALID_SHARE * length as f64
    }
}

fn cumulative(values: impl Iterator<Item = f64>) -> Vec<f64> {
    std::iter::once(0.0)
        .chain(values.scan(0.0, |total, value| {
            *total += value;
            Some(*total)
        }))
        .collect()
}

fn fill_holes(values: &[f32], measured: &[bool]) -> Vec<f64> {
    let mut filled: Vec<f64> = values.iter().map(|value| f64::from(*value)).collect();
    let mut index = 0;
    while index < filled.len() {
        if measured[index] {
            index += 1;
            continue;
        }
        let start = index;
        while index < filled.len() && !measured[index] {
            index += 1;
        }
        let before = start.checked_sub(1).map(|at| filled[at]);
        let after = filled.get(index).copied();
        let mean = match (before, after) {
            (Some(a), Some(b)) => (a + b) / 2.0,
            (Some(value), None) | (None, Some(value)) => value,
            (None, None) => 0.0,
        };
        filled[start..index].fill(mean);
    }
    filled
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_hole(values: &[f32], hole: std::ops::Range<usize>) -> Series {
        let mut series = Series::complete(100.0, 10.0, values.to_vec());
        series.valid[hole].fill(false);
        series
    }

    #[test]
    fn holes_take_the_mean_of_their_neighbours() {
        let prepared = Prepared::new(&with_hole(&[2.0, 0.0, 0.0, 4.0], 1..3));

        assert_eq!(prepared.filled, [2.0, 3.0, 3.0, 4.0]);
    }

    #[test]
    fn holes_at_the_ends_copy_the_nearest_value() {
        assert_eq!(
            Prepared::new(&with_hole(&[0.0, 5.0], 0..1)).filled,
            [5.0, 5.0]
        );
        assert_eq!(
            Prepared::new(&with_hole(&[5.0, 0.0], 1..2)).filled,
            [5.0, 5.0]
        );
    }

    #[test]
    fn a_series_with_nothing_valid_is_zero() {
        assert_eq!(
            Prepared::new(&with_hole(&[9.0, 9.0], 0..2)).filled,
            [0.0, 0.0]
        );
    }

    #[test]
    fn values_that_are_not_finite_count_as_holes() {
        let values = vec![1.0, f32::NAN, f32::INFINITY, 3.0];
        let prepared = Prepared::new(&Series::complete(0.0, 1.0, values));

        assert_eq!(prepared.filled, [1.0, 2.0, 2.0, 3.0]);
        assert!(!prepared.has_data(0, 4));
        assert!(prepared.stats(0, 4).1.is_finite());
    }

    #[test]
    fn statistics_cover_the_window() {
        let prepared = Prepared::new(&Series::complete(0.0, 1.0, vec![1.0, 2.0, 3.0, 9.0]));

        let (mean, deviation) = prepared.stats(0, 3);

        assert!((mean - 2.0).abs() < 1e-12);
        assert!((deviation - (2f64 / 3.0).sqrt()).abs() < 1e-12);
    }

    #[test]
    fn windows_with_too_many_holes_have_no_data() {
        let prepared = Prepared::new(&with_hole(&[1.0; 20], 0..3));

        assert!(!prepared.has_data(0, 20));
        assert!(prepared.has_data(2, 18));
        assert!(!prepared.has_data(15, 10));
    }

    #[test]
    fn times_and_indexes_convert_both_ways() {
        let prepared = Prepared::new(&Series::complete(100.0, 10.0, vec![0.0; 50]));

        assert_eq!(prepared.index_of(101.0), Some(10));
        assert_eq!(prepared.index_of(99.0), None);
        assert_eq!(prepared.time_of(10), 101.0);
        assert_eq!(prepared.end_time(), 105.0);
        assert_eq!(prepared.samples(2.5), 25);
        assert!(!prepared.is_empty());
    }
}

//! Normalised cross-correlation of a reference window against a stretch of a series.

use super::series::Prepared;
use realfft::RealFftPlanner;
use realfft::num_complex::Complex;
use std::fmt;
use std::ops::Range;

/// Positions whose deviation is at most this share of the reference's score 0 (D-05).
const VARIANCE_FLOOR: f64 = 0.3;
/// A reference flatter than this is silence, which is no evidence either way (D-05).
pub(crate) const MIN_REFERENCE_DEVIATION: f64 = 0.05;

/// What a search found. "No data" is not "no match", and never counts as evidence (D-05).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Match {
    /// The reference or its target has too little to judge.
    NoData,
    /// The audio matched nothing in range.
    NoMatch,
    /// The audio matched this far back.
    Lag {
        /// Seconds back.
        lag: f64,
        /// Score of the match.
        score: f32,
    },
}

/// FFT plans. One per thread, as plans are not shared.
pub struct Correlator {
    planner: RealFftPlanner<f64>,
}

impl fmt::Debug for Correlator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Correlator").finish_non_exhaustive()
    }
}

impl Default for Correlator {
    fn default() -> Self {
        Self {
            planner: RealFftPlanner::new(),
        }
    }
}

impl Correlator {
    /// Scores the window `reference` at each start position in `positions` of the same series.
    pub fn scores(
        &mut self,
        data: &Prepared,
        reference: Range<usize>,
        positions: Range<usize>,
    ) -> Vec<f32> {
        self.scores_across(data, reference, data, positions)
    }

    /// Scores a window of `source` at each start position in `positions` of `data`.
    ///
    /// The numerator comes from an FFT and the denominators from cumulative sums. Positions
    /// that are too flat or have too many holes score 0.
    pub fn scores_across(
        &mut self,
        source: &Prepared,
        reference: Range<usize>,
        data: &Prepared,
        positions: Range<usize>,
    ) -> Vec<f32> {
        let length = reference.len();
        let (mean, deviation) = source.stats(reference.start, length);
        if deviation <= 0.0 || positions.is_empty() {
            return vec![0.0; positions.len()];
        }
        let normalised: Vec<f64> = source.filled[reference]
            .iter()
            .map(|value| (value - mean) / deviation)
            .collect();
        let segment = &data.filled[positions.start..positions.end + length - 1];
        let numerators = self.correlate(segment, &normalised);
        (0..positions.len())
            .map(|offset| {
                let position = positions.start + offset;
                let (_, spread) = data.stats(position, length);
                if spread <= VARIANCE_FLOOR * deviation || !data.has_data(position, length) {
                    return 0.0;
                }
                (numerators[offset] / (length as f64 * spread)).clamp(-1.0, 1.0) as f32
            })
            .collect()
    }

    fn correlate(&mut self, segment: &[f64], reference: &[f64]) -> Vec<f64> {
        let size = (segment.len() + reference.len()).next_power_of_two();
        let forward = self.planner.plan_fft_forward(size);
        let inverse = self.planner.plan_fft_inverse(size);
        let spectrum = |signal: &[f64]| {
            let mut input = forward.make_input_vec();
            input[..signal.len()].copy_from_slice(signal);
            let mut output = forward.make_output_vec();
            forward
                .process(&mut input, &mut output)
                .expect("buffers come from the plan");
            output
        };
        let target = spectrum(segment);
        let window = spectrum(reference);
        let mut product: Vec<Complex<f64>> = target
            .iter()
            .zip(&window)
            .map(|(a, b)| a * b.conj())
            .collect();
        let last = product.len() - 1;
        product[0].im = 0.0;
        product[last].im = 0.0;
        let mut result = inverse.make_output_vec();
        inverse
            .process(&mut product, &mut result)
            .expect("buffers come from the plan");
        let scale = 1.0 / size as f64;
        result.iter_mut().for_each(|value| *value *= scale);
        result
    }
}

/// Looks for the window at `time` between `lags` seconds back.
///
/// With `expected_lag`, a target that falls in a recording gap yields `NoData`, otherwise every
/// outage would raise a false change one cycle later.
pub fn search(
    correlator: &mut Correlator,
    data: &Prepared,
    time: f64,
    window_seconds: f64,
    lags: Range<f64>,
    threshold: f32,
    expected_lag: Option<f64>,
) -> Match {
    let length = data.samples(window_seconds);
    let Some(index) = data.index_of(time) else {
        return Match::NoData;
    };
    if index + length > data.len() || !data.has_data(index, length) {
        return Match::NoData;
    }
    if data.stats(index, length).1 < MIN_REFERENCE_DEVIATION {
        return Match::NoData;
    }
    let Some(last) = index.checked_sub(data.samples(lags.start)) else {
        return Match::NoData;
    };
    if let Some(lag) = expected_lag {
        let target = index.checked_sub(data.samples(lag));
        if !target.is_some_and(|start| data.has_data(start, length)) {
            return Match::NoData;
        }
    }
    let first = index.saturating_sub(data.samples(lags.end));
    let scores = correlator.scores(data, index..index + length, first..last + 1);
    let best = scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(offset, score)| (offset, *score));
    match best {
        Some((offset, score)) if score >= threshold => Match::Lag {
            lag: (index - (first + offset)) as f64 / data.rate,
            score,
        },
        _ => Match::NoMatch,
    }
}

#[cfg(test)]
mod tests {
    use super::super::series::Series;
    use super::super::synthetic::{content, prepared};
    use super::*;

    #[test]
    fn a_copy_scores_one_at_its_position() {
        let values = content(1, 300.0);
        let data = prepared(&values);

        let scores = Correlator::default().scores(&data, 1000..1600, 0..900);

        let best = scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap();
        assert!(*best.1 < 1.0 + f32::EPSILON);
        assert!(best.1 < &0.9, "{}", best.1);

        let mut repeated = values[..600].to_vec();
        repeated.extend(content(2, 60.0));
        repeated.extend_from_slice(&values[..600]);
        let data = prepared(&repeated);
        let scores = Correlator::default().scores(&data, 1200..1800, 0..600);
        assert!((scores[0] - 1.0).abs() < 1e-5, "{}", scores[0]);
    }

    #[test]
    fn scaling_and_offset_do_not_change_the_score() {
        let values = content(3, 60.0);
        let shifted: Vec<f32> = values.iter().map(|value| value * 0.5 + 3.0).collect();
        let data = prepared(&[values.clone(), shifted].concat());

        let scores = Correlator::default().scores(&data, 600..1200, 0..1);

        assert!((scores[0] - 1.0).abs() < 1e-5, "{}", scores[0]);
    }

    #[test]
    fn scores_stay_bounded_on_any_input() {
        let cases = [
            content(4, 120.0),
            vec![0.0; 1200],
            (0..1200)
                .map(|index| if index % 2 == 0 { 0.0 } else { 1e6 })
                .collect(),
            (0..1200).map(|index| (index % 100) as f32 * 1e-6).collect(),
            [vec![0.0; 600], content(5, 60.0)].concat(),
        ];
        for values in cases {
            let data = prepared(&values);
            let scores = Correlator::default().scores(&data, 300..500, 0..1000);
            assert!(scores.iter().all(|score| score.abs() <= 1.0));
        }
    }

    #[test]
    fn silent_positions_score_zero() {
        let values = [vec![1.0; 600], content(6, 60.0)].concat();
        let data = prepared(&values);

        let scores = Correlator::default().scores(&data, 600..1200, 0..1);

        assert_eq!(scores, [0.0]);
    }

    #[test]
    fn a_silent_reference_is_no_data() {
        let data = prepared(&[content(7, 200.0), vec![2.0; 1200]].concat());

        let result = search(
            &mut Correlator::default(),
            &data,
            201.0,
            60.0,
            1.0..100.0,
            0.6,
            None,
        );

        assert_eq!(result, Match::NoData);
    }

    #[test]
    fn a_target_in_a_gap_is_no_data() {
        let values = content(8, 300.0);
        let mut series = Series::complete(0.0, 10.0, values.clone());
        series.valid[100..700].fill(false);
        let data = Prepared::new(&series);

        let result = search(
            &mut Correlator::default(),
            &data,
            200.0,
            60.0,
            10.0..190.0,
            0.6,
            Some(190.0),
        );

        assert_eq!(result, Match::NoData);
    }

    #[test]
    fn unrelated_audio_is_no_match() {
        let data = prepared(&[content(9, 200.0), content(10, 100.0)].concat());

        let result = search(
            &mut Correlator::default(),
            &data,
            210.0,
            60.0,
            10.0..200.0,
            0.8,
            None,
        );

        assert_eq!(result, Match::NoMatch);
    }

    #[test]
    fn a_repeat_reports_its_lag() {
        let first = content(11, 100.0);
        let data = prepared(&[first.clone(), first].concat());

        let result = search(
            &mut Correlator::default(),
            &data,
            120.0,
            60.0,
            10.0..119.0,
            0.8,
            None,
        );

        let Match::Lag { lag, score } = result else {
            panic!("{result:?}");
        };
        assert!((lag - 100.0).abs() < 0.11);
        assert!(score > 0.99);
    }

    #[test]
    fn a_window_past_the_end_or_before_the_start_is_no_data() {
        let data = prepared(&content(12, 100.0));
        let mut correlator = Correlator::default();

        for time in [95.0, -5.0] {
            let result = search(&mut correlator, &data, time, 60.0, 1.0..10.0, 0.5, None);
            assert_eq!(result, Match::NoData);
        }
        let early = search(&mut correlator, &data, 5.0, 60.0, 50.0..60.0, 0.5, None);
        assert_eq!(early, Match::NoData);
    }
}

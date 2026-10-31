//! Estimating the loop period, without reporting a multiple of it (D-02, D-03).

use super::ncc::Correlator;
use super::series::Prepared;
use rayon::prelude::*;

/// Search parameters. Defaults come from the measured streams.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PeriodConfig {
    /// Smallest candidate, in seconds.
    pub min_period: f64,
    /// Largest candidate, in seconds.
    pub max_period: f64,
    /// Reference length, in seconds.
    pub reference_seconds: f64,
    /// Distance between references, in seconds.
    pub reference_spacing: f64,
    /// Number of references.
    pub references: usize,
    /// Score a peak must reach.
    pub threshold: f32,
    /// How close lags must be to agree, in seconds.
    pub tolerance: f64,
    /// References that must agree.
    pub min_agreeing: usize,
}

impl Default for PeriodConfig {
    fn default() -> Self {
        Self {
            min_period: 1800.0,
            max_period: 86_400.0,
            reference_seconds: 600.0,
            reference_spacing: 1800.0,
            references: 6,
            threshold: 0.9,
            tolerance: 2.0,
            min_agreeing: 4,
        }
    }
}

/// What the estimate found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PeriodEstimate {
    /// A period, confirmed by several references.
    Found {
        /// Seconds.
        period: f64,
        /// References that agreed.
        agreeing: usize,
        /// Whether a match also exists at twice the period. Advisory, because within two
        /// periods of a change a correct period has none (D-02).
        confirmed_by_double: bool,
    },
    /// No period, with the longest one the history could have shown (D-03).
    NoPeriod {
        /// Seconds.
        testable_up_to: f64,
    },
}

/// Estimates the period from references ending near the tail of `data`.
pub fn estimate_period(data: &Prepared, config: &PeriodConfig) -> PeriodEstimate {
    let length = data.samples(config.reference_seconds);
    let spacing = data.samples(config.reference_spacing);
    let span = length + spacing * config.references.saturating_sub(1);
    let history = data.len() as f64 / data.rate;
    let testable_up_to = (history - span as f64 / data.rate).max(0.0);
    let starts: Vec<usize> = (0..config.references)
        .filter_map(|index| data.len().checked_sub(length + index * spacing))
        .collect();
    let peaks: Vec<Vec<f64>> = starts
        .par_iter()
        .map_init(Correlator::default, |correlator, start| {
            reference_peaks(correlator, data, *start, length, config)
        })
        .collect();
    let mut smallest: Vec<f64> = peaks
        .iter()
        .filter_map(|lags| lags.first().copied())
        .collect();
    smallest.sort_by(f64::total_cmp);
    let Some(median) = smallest.get(smallest.len() / 2).copied() else {
        return no_period(testable_up_to, config);
    };
    let agreeing = smallest
        .iter()
        .filter(|lag| (*lag - median).abs() <= config.tolerance)
        .count();
    if agreeing < config.min_agreeing {
        return no_period(testable_up_to, config);
    }
    let double = 2.0 * median;
    PeriodEstimate::Found {
        period: median,
        agreeing,
        confirmed_by_double: peaks
            .iter()
            .flatten()
            .any(|lag| (lag - double).abs() <= 2.0 * config.tolerance),
    }
}

fn no_period(testable_up_to: f64, config: &PeriodConfig) -> PeriodEstimate {
    PeriodEstimate::NoPeriod {
        testable_up_to: testable_up_to.min(config.max_period),
    }
}

/// Returns the lag of each peak above the threshold, smallest first.
fn reference_peaks(
    correlator: &mut Correlator,
    data: &Prepared,
    start: usize,
    length: usize,
    config: &PeriodConfig,
) -> Vec<f64> {
    let Some(last) = start.checked_sub(data.samples(config.min_period)) else {
        return Vec::new();
    };
    let first = start.saturating_sub(data.samples(2.0 * config.max_period));
    let scores = correlator.scores(data, start..start + length, first..last + 1);
    let longest = data.samples(config.max_period);
    let mut lags: Vec<usize> = clusters(&scores, config.threshold)
        .map(|offset| start - (first + offset))
        .collect();
    lags.sort_unstable();
    let shortest = lags.first().copied();
    lags.into_iter()
        .filter(|lag| *lag <= longest || shortest.is_some_and(|value| *lag <= 2 * value + 40))
        .map(|lag| lag as f64 / data.rate)
        .collect()
}

/// Yields the best offset of each stretch of scores above the threshold.
pub(super) fn clusters(scores: &[f32], threshold: f32) -> impl Iterator<Item = usize> + '_ {
    let mut offset = 0;
    std::iter::from_fn(move || {
        while offset < scores.len() && scores[offset] < threshold {
            offset += 1;
        }
        if offset == scores.len() {
            return None;
        }
        let start = offset;
        while offset < scores.len() && scores[offset] >= threshold {
            offset += 1;
        }
        (start..offset).max_by(|a, b| scores[*a].total_cmp(&scores[*b]))
    })
}

#[cfg(test)]
mod tests {
    use super::super::synthetic::{content, prepared};
    use super::*;

    const CONFIG: PeriodConfig = PeriodConfig {
        min_period: 300.0,
        max_period: 8000.0,
        reference_seconds: 120.0,
        reference_spacing: 120.0,
        references: 6,
        threshold: 0.9,
        tolerance: 2.0,
        min_agreeing: 4,
    };

    fn found(estimate: PeriodEstimate) -> (f64, bool) {
        match estimate {
            PeriodEstimate::Found {
                period,
                confirmed_by_double,
                ..
            } => (period, confirmed_by_double),
            PeriodEstimate::NoPeriod { .. } => panic!("{estimate:?}"),
        }
    }

    #[test]
    fn finds_a_known_period() {
        let cycle = content(1, 1500.0);
        let data = prepared(&cycle.repeat(4));

        let (period, doubled) = found(estimate_period(&data, &CONFIG));

        assert!((period - 1500.0).abs() < 0.11, "{period}");
        assert!(doubled);
    }

    #[test]
    fn never_reports_a_multiple_of_the_period() {
        let cycle = content(2, 1000.0);
        let data = prepared(&cycle.repeat(9));

        let (period, doubled) = found(estimate_period(&data, &CONFIG));

        assert!((period - 1000.0).abs() < 0.11, "{period}");
        assert!(doubled);
    }

    #[test]
    fn a_period_right_after_a_change_needs_no_double() {
        let old = content(3, 3000.0);
        let new = content(4, 1500.0);
        let data = prepared(&[old, new.clone(), new].concat());

        let (period, doubled) = found(estimate_period(&data, &CONFIG));

        assert!((period - 1500.0).abs() < 0.11, "{period}");
        assert!(!doubled);
    }

    #[test]
    fn audio_that_never_repeats_has_no_period() {
        let data = prepared(&content(5, 6000.0));

        let estimate = estimate_period(&data, &CONFIG);

        assert!(matches!(estimate, PeriodEstimate::NoPeriod { .. }));
    }

    #[test]
    fn no_period_states_what_the_history_could_show() {
        let data = prepared(&content(6, 2000.0));

        let PeriodEstimate::NoPeriod { testable_up_to } = estimate_period(&data, &CONFIG) else {
            panic!("expected no period");
        };

        assert!((testable_up_to - 1280.0).abs() < 0.5, "{testable_up_to}");
    }

    #[test]
    fn a_series_shorter_than_the_references_has_no_period() {
        let data = prepared(&content(7, 60.0));

        assert_eq!(
            estimate_period(&data, &CONFIG),
            PeriodEstimate::NoPeriod {
                testable_up_to: 0.0
            }
        );
    }

    #[test]
    fn repeated_material_inside_a_cycle_does_not_win() {
        let clip = content(8, 40.0);
        let parts: Vec<Vec<f32>> = (0..4)
            .flat_map(|index| [content(20 + index, 360.0), clip.clone()])
            .collect();
        let cycle = parts.concat();
        let data = prepared(&cycle.repeat(4));

        let (period, _) = found(estimate_period(&data, &CONFIG));

        assert!((period - 1600.0).abs() < 0.11, "{period}");
    }

    #[test]
    fn clusters_yield_the_best_of_each_stretch() {
        let scores = [0.1, 0.95, 0.99, 0.91, 0.2, 0.92];

        assert_eq!(clusters(&scores, 0.9).collect::<Vec<_>>(), [2, 5]);
    }
}

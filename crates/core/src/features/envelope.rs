//! Resampling to 2 kHz, then RMS over 200 samples and `log1p`.

use std::collections::VecDeque;
use std::f64::consts::PI;

/// Values per second of the envelope.
pub const BIN_RATE: f64 = 10.0;

const TARGET_RATE: u64 = 2_000;
const SAMPLES_PER_BIN: u64 = 200;
const ZERO_CROSSINGS: f64 = 16.0;
const CUTOFF: f64 = 0.97;
const S16_SCALE: f64 = 32_768.0;
const S16_MAX: f64 = 32_767.0;

/// Consumes mono samples at known ticks and produces bins on a fixed grid.
#[derive(Debug, Clone)]
pub struct EnvelopeCursor {
    ticks_per_target_numerator: u64,
    ticks_per_target_denominator: u64,
    half_width: usize,
    /// One row of `2 * half_width + 1` taps per sub-sample phase.
    filter: Vec<f32>,
    origin: u64,
    buffer: VecDeque<f32>,
    buffer_tick: u64,
    next_target: u64,
    bin_energy: f64,
}

impl EnvelopeCursor {
    /// Starts at `first_tick`, with bin zero at tick `origin`.
    ///
    /// The first bin is the first whole bin at or after `first_tick`.
    pub fn new(input_rate: u32, origin: u64, first_tick: u64) -> Self {
        let rate = u64::from(input_rate);
        let divisor = gcd(rate, TARGET_RATE);
        let numerator = rate / divisor;
        let denominator = TARGET_RATE / divisor;
        let half_width = half_width(rate);
        let elapsed = first_tick.saturating_sub(origin);
        let first_target = (elapsed * denominator).div_ceil(numerator);
        Self {
            ticks_per_target_numerator: numerator,
            ticks_per_target_denominator: denominator,
            half_width,
            filter: build_filter(rate, denominator, half_width),
            origin,
            buffer: VecDeque::new(),
            buffer_tick: first_tick,
            next_target: first_target.next_multiple_of(SAMPLES_PER_BIN),
            bin_energy: 0.0,
        }
    }

    /// Returns the grid index of the next bin to complete.
    pub fn next_bin(&self) -> u64 {
        self.next_target / SAMPLES_PER_BIN
    }

    /// Returns the tick just past the last sample received.
    pub fn end_tick(&self) -> u64 {
        self.buffer_tick + self.buffer.len() as u64
    }

    /// Adds samples that follow on from the previous ones, and appends completed bins.
    pub fn push(&mut self, samples: &[f32], bins: &mut Vec<f32>) {
        self.buffer.extend(samples);
        self.buffer.make_contiguous();
        loop {
            let scaled = self.next_target * self.ticks_per_target_numerator;
            let position = self.origin + scaled / self.ticks_per_target_denominator;
            let phase = scaled % self.ticks_per_target_denominator;
            if position + self.half_width as u64 + 1 >= self.end_tick() {
                break;
            }
            let value = self.convolve(position, phase as usize);
            self.accept(value, bins);
            self.next_target += 1;
        }
        self.discard_used();
    }

    fn convolve(&self, position: u64, phase: usize) -> f32 {
        let taps = 2 * self.half_width + 1;
        let row = &self.filter[phase * taps..(phase + 1) * taps];
        let first_wanted = position.saturating_sub(self.half_width as u64);
        let skipped_before_input = (self.half_width as u64).saturating_sub(position);
        let missing = self.buffer_tick.saturating_sub(first_wanted) + skipped_before_input;
        let start = first_wanted.saturating_sub(self.buffer_tick) as usize;
        let (samples, _) = self.buffer.as_slices();
        row[missing as usize..]
            .iter()
            .zip(&samples[start..])
            .map(|(tap, sample)| tap * sample)
            .sum()
    }

    fn accept(&mut self, value: f32, bins: &mut Vec<f32>) {
        let quantised = (f64::from(value) * S16_SCALE)
            .round()
            .clamp(-S16_SCALE, S16_MAX);
        self.bin_energy += quantised * quantised;
        if (self.next_target + 1).is_multiple_of(SAMPLES_PER_BIN) {
            let rms = (self.bin_energy / SAMPLES_PER_BIN as f64).sqrt();
            bins.push(rms.ln_1p() as f32);
            self.bin_energy = 0.0;
        }
    }

    fn discard_used(&mut self) {
        let scaled = self.next_target * self.ticks_per_target_numerator;
        let position = self.origin + scaled / self.ticks_per_target_denominator;
        let keep_from = position.saturating_sub(self.half_width as u64);
        let used = keep_from.saturating_sub(self.buffer_tick);
        let drained = (used as usize).min(self.buffer.len());
        self.buffer.drain(..drained);
        self.buffer_tick += drained as u64;
    }
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn half_width(input_rate: u64) -> usize {
    (ZERO_CROSSINGS / normalised_cutoff(input_rate)).ceil() as usize
}

fn normalised_cutoff(input_rate: u64) -> f64 {
    CUTOFF * TARGET_RATE as f64 / input_rate as f64
}

// A Blackman-windowed sinc per phase, each normalised to unit gain at DC.
fn build_filter(input_rate: u64, phases: u64, half_width: usize) -> Vec<f32> {
    let cutoff = normalised_cutoff(input_rate);
    let window_half = ZERO_CROSSINGS / cutoff;
    (0..phases)
        .flat_map(|phase| {
            let offset = phase as f64 / phases as f64;
            let taps: Vec<f64> = (-(half_width as i64)..=half_width as i64)
                .map(|index| windowed_sinc(index as f64 - offset, cutoff, window_half))
                .collect();
            let gain: f64 = taps.iter().sum();
            taps.into_iter()
                .map(move |tap| (tap / gain) as f32)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn windowed_sinc(x: f64, cutoff: f64, window_half: f64) -> f64 {
    if x.abs() > window_half {
        return 0.0;
    }
    let turn = PI * x / window_half;
    let window = 0.42 + 0.5 * turn.cos() + 0.08 * (2.0 * turn).cos();
    let argument = PI * cutoff * x;
    let sinc = if argument == 0.0 {
        1.0
    } else {
        argument.sin() / argument
    };
    sinc * window
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44_100;

    fn sine(frequency: f64, amplitude: f64, samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|index| {
                let time = index as f64 / f64::from(RATE);
                (amplitude * (2.0 * PI * frequency * time).sin()) as f32
            })
            .collect()
    }

    fn bins_of(samples: &[f32]) -> Vec<f32> {
        let mut bins = Vec::new();
        EnvelopeCursor::new(RATE, 0, 0).push(samples, &mut bins);
        bins
    }

    #[test]
    fn makes_ten_bins_per_second() {
        let bins = bins_of(&vec![0.1; RATE as usize]);

        assert!((9..=10).contains(&bins.len()), "{}", bins.len());
    }

    #[test]
    fn silence_is_exactly_zero() {
        assert!(
            bins_of(&vec![0.0; RATE as usize])
                .iter()
                .all(|bin| *bin == 0.0)
        );
    }

    #[test]
    fn a_low_tone_keeps_its_rms() {
        let amplitude = 0.25;
        let bins = bins_of(&sine(100.0, amplitude, RATE as usize));

        let expected = (amplitude * S16_SCALE / 2f64.sqrt()).ln_1p() as f32;
        assert!((bins[5] - expected).abs() < 0.01, "{} {expected}", bins[5]);
    }

    #[test]
    fn a_tone_above_nyquist_is_rejected() {
        let bins = bins_of(&sine(5_000.0, 0.25, RATE as usize));

        assert!(bins[5] < 1.0, "{}", bins[5]);
    }

    #[test]
    fn any_split_into_calls_gives_the_same_bins() {
        let samples = sine(321.0, 0.3, 3 * RATE as usize);
        let whole = bins_of(&samples);

        for chunk in [1, 7, 1024, 4096, 220_500] {
            let mut cursor = EnvelopeCursor::new(RATE, 0, 0);
            let mut bins = Vec::new();
            for part in samples.chunks(chunk) {
                cursor.push(part, &mut bins);
            }
            assert_eq!(bins, whole, "chunk {chunk}");
        }
    }

    #[test]
    fn the_buffer_stays_bounded() {
        let mut cursor = EnvelopeCursor::new(RATE, 0, 0);
        let mut bins = Vec::new();

        for _ in 0..200 {
            cursor.push(&[0.1; 1024], &mut bins);
        }

        assert!(cursor.buffer.len() < 4 * cursor.half_width);
    }

    #[test]
    fn end_tick_counts_every_sample() {
        let mut cursor = EnvelopeCursor::new(RATE, 0, 1_000);

        cursor.push(&vec![0.0; 50_000], &mut Vec::new());

        assert_eq!(cursor.end_tick(), 51_000);
    }

    #[test]
    fn the_first_bin_is_the_next_whole_one_on_the_grid() {
        assert_eq!(EnvelopeCursor::new(RATE, 0, 0).next_bin(), 0);
        assert_eq!(EnvelopeCursor::new(RATE, 0, 1).next_bin(), 1);
        assert_eq!(EnvelopeCursor::new(RATE, 0, 4_410).next_bin(), 1);
        assert_eq!(EnvelopeCursor::new(RATE, 0, 4_411).next_bin(), 2);
    }

    #[test]
    fn the_origin_moves_the_grid() {
        assert_eq!(EnvelopeCursor::new(RATE, 1_000, 1_000).next_bin(), 0);
        assert_eq!(EnvelopeCursor::new(RATE, 1_000, 5_410).next_bin(), 1);
    }

    #[test]
    fn a_late_start_on_the_grid_equals_the_same_bins_from_the_beginning() {
        let samples = sine(321.0, 0.3, 3 * RATE as usize);
        let whole = bins_of(&samples);
        let start = 2 * 4_410;

        let mut bins = Vec::new();
        EnvelopeCursor::new(RATE, 0, start as u64).push(&samples[start..], &mut bins);

        // The first bins lack left context of the filter, and later ones must agree.
        assert_eq!(bins[3..], whole[2 + 3..2 + bins.len()]);
    }

    #[test]
    fn every_phase_has_unit_gain() {
        let half = half_width(44_100);
        let filter = build_filter(44_100, 20, half);

        for row in filter.chunks(2 * half + 1) {
            let gain: f32 = row.iter().sum();
            assert!((gain - 1.0).abs() < 1e-5, "{gain}");
        }
    }

    #[test]
    fn clipping_stays_in_the_s16_range() {
        let bins = bins_of(&vec![4.0; RATE as usize]);

        assert!(bins[5] <= (S16_SCALE).ln_1p() as f32 + 1e-3, "{}", bins[5]);
    }
}

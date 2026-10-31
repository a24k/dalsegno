//! Reproducible test signals at 10 values per second.

use super::series::{Prepared, Series};

const RATE: f64 = 10.0;
const SMOOTHING: f32 = 0.8;
const LEVEL: f32 = 8.0;

/// Smoothed noise, which matches only itself. Different seeds are unrelated.
pub(crate) fn content(seed: u64, seconds: f64) -> Vec<f32> {
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut level = 0.0;
    (0..(seconds * RATE) as usize)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let noise = (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5;
            level = SMOOTHING * level + (1.0 - SMOOTHING) * noise * 8.0;
            LEVEL + level
        })
        .collect()
}

pub(crate) fn prepared(values: &[f32]) -> Prepared {
    Prepared::new(&Series::complete(0.0, RATE, values.to_vec()))
}

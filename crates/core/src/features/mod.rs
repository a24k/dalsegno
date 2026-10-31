//! Turning audio fragments into the 10 Hz envelope (FR-N1, D-07).
//!
//! The output is a pure function of the decoded samples and their position on the grid, so
//! any split into calls gives the same values, which is what makes incremental equal batch.

mod decoder;
mod envelope;

pub use decoder::AacStream;
pub use envelope::{BIN_RATE, EnvelopeCursor};

use crate::container::Mp4Fragment;

/// Why audio could not be processed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FeatureError {
    /// The decoder rejected the stream's configuration.
    #[error("unsupported audio: {0}")]
    Unsupported(String),
    /// The decoder rejected a packet.
    #[error("undecodable audio packet: {0}")]
    Decode(String),
    /// The fragment's clock does not count samples, so positions would be wrong.
    #[error("timescale {timescale} differs from sample rate {sample_rate}")]
    Timescale {
        /// Ticks per second of the fragment.
        timescale: u32,
        /// Samples per second of the audio.
        sample_rate: u32,
    },
}

/// The bins completed while processing one fragment.
#[derive(Debug, Clone, PartialEq)]
pub struct Bins {
    /// Grid index of the first value.
    pub first_bin: u64,
    /// The values, contiguous on the grid.
    pub values: Vec<f32>,
}

/// One decoder and one cursor, which last for a contiguous run of fragments.
#[derive(Debug)]
pub struct EnvelopeExtractor {
    decoder: AacStream,
    cursor: EnvelopeCursor,
    skip_before_bin: u64,
}

impl EnvelopeExtractor {
    /// Starts at `first`, on the grid anchored to stream time zero (D-07).
    pub fn new(first: &Mp4Fragment<'_>) -> Result<Self, FeatureError> {
        Self::with_grid_origin(first, 0)
    }

    /// Starts at `first`, with bin zero at tick `origin`.
    pub fn with_grid_origin(first: &Mp4Fragment<'_>, origin: u64) -> Result<Self, FeatureError> {
        let decoder = AacStream::new(first.audio_config)?;
        let sample_rate = decoder.sample_rate();
        if sample_rate != first.timescale {
            return Err(FeatureError::Timescale {
                timescale: first.timescale,
                sample_rate,
            });
        }
        Ok(Self {
            decoder,
            cursor: EnvelopeCursor::new(sample_rate, origin, first.start_ticks),
            skip_before_bin: 0,
        })
    }

    /// Discards bins before `bin`, which already exist when a run resumes.
    pub fn skip_before(&mut self, bin: u64) {
        self.skip_before_bin = bin;
    }

    /// Returns the tick just past the last decoded sample.
    pub fn end_tick(&self) -> u64 {
        self.cursor.end_tick()
    }

    /// Decodes one fragment and returns the bins it completed.
    pub fn process(&mut self, fragment: &Mp4Fragment<'_>) -> Result<Bins, FeatureError> {
        let first_bin = self.cursor.next_bin();
        let mut mono = Vec::new();
        for packet in &fragment.packets {
            self.decoder.decode(packet, &mut mono)?;
        }
        let mut values = Vec::new();
        self.cursor.push(&mono, &mut values);
        let skipped = self.skip_before_bin.saturating_sub(first_bin) as usize;
        let kept = skipped.min(values.len());
        Ok(Bins {
            first_bin: first_bin + kept as u64,
            values: values.split_off(kept),
        })
    }
}

//! Readers for single media fragments, working on in-memory buffers.
//!
//! Malformed input yields an error and never a panic, because fragments come from the network.

mod mp4;
mod webm;

pub use mp4::{Mp4Fragment, parse_mp4_fragment};
pub use webm::parse_webm_start_time;

const EBML_MAGIC: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

/// When a fragment plays, read from its bytes (FR-A9).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FragmentTiming {
    /// Stream time of the first sample, in seconds.
    pub start_time: f64,
    /// Duration in seconds, which only mp4 fragments carry.
    pub duration: Option<f64>,
}

/// Reads the timing of a WebM fragment or an audio mp4 fragment.
pub fn read_timing(data: &[u8]) -> Result<FragmentTiming, ContainerError> {
    if data.starts_with(&EBML_MAGIC) {
        return Ok(FragmentTiming {
            start_time: parse_webm_start_time(data)?,
            duration: None,
        });
    }
    let fragment = parse_mp4_fragment(data)?;
    Ok(FragmentTiming {
        start_time: fragment.start_time(),
        duration: Some(fragment.duration()),
    })
}

/// Why a fragment could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContainerError {
    /// The buffer ends inside the named structure.
    #[error("truncated {0}")]
    Truncated(&'static str),
    /// A required structure is absent.
    #[error("missing {0}")]
    Missing(&'static str),
    /// The structure is valid but uses a feature this reader does not handle.
    #[error("unsupported {0}")]
    Unsupported(&'static str),
}

//! Readers for single media fragments, working on in-memory buffers.
//!
//! Malformed input yields an error and never a panic, because fragments come from the network.

mod mp4;
mod webm;

pub use mp4::{Mp4Fragment, parse_mp4_fragment};
pub use webm::parse_webm_start_time;

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

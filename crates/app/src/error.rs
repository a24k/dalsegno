//! Errors of the acquisition use cases. Messages are safe to log and show.

use dalsegno_core::stream::IpFamily;
use dalsegno_io::http::TransportError;
use std::io;

/// Why an acquisition use case stopped.
#[derive(Debug, thiserror::Error)]
pub enum AcquireError {
    /// The stream is gone. This concerns only this stream (D-26).
    #[error("stream unavailable: {0}")]
    Unavailable(String),
    /// Every family the policy allows has failed.
    #[error("no usable IP family left, last tried {0}")]
    FamilyExhausted(IpFamily),
    /// yt-dlp kept failing.
    #[error("URL resolution failed, yt-dlp may need updating: {0}")]
    Resolve(String),
    /// The request itself makes no sense.
    #[error("invalid request: {0}")]
    InvalidRequest(&'static str),
    /// The origin did not report its head.
    #[error("the origin did not report a head sequence number")]
    NoHead,
    /// The disk refused a write.
    #[error("fragment storage failed: {0}")]
    Store(#[from] io::Error),
    /// The HTTP client failed outside a fragment request.
    #[error("HTTP client failed: {0}")]
    Transport(#[from] TransportError),
    /// A background task ended abnormally.
    #[error("a background task stopped unexpectedly")]
    Task,
}

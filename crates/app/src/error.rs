//! Errors of the acquisition use cases. Messages are safe to log and show.

use dalsegno_core::export::{CutError, NameError};
use dalsegno_core::stream::IpFamily;
use dalsegno_io::http::TransportError;
use dalsegno_io::plex::PlexError;
use dalsegno_io::tools::ToolError;
use dalsegno_io::transfer::TransferError;
use dalsegno_store::database::StoreError;
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
    /// The database refused an operation.
    #[error("{0}")]
    Database(#[from] StoreError),
    /// The stream has not been added yet.
    #[error("stream {0} is not registered, run add first")]
    NotRegistered(String),
    /// An envelope could not be built or read.
    #[error("envelope: {0}")]
    Envelope(String),
    /// The probe found no usable audio.
    #[error("no audio format passed its checks")]
    NoAudio,
    /// The HTTP client failed outside a fragment request.
    #[error("HTTP client failed: {0}")]
    Transport(#[from] TransportError),
    /// An external tool failed.
    #[error("{0}")]
    Tool(#[from] ToolError),
    /// A cut cannot go ahead.
    #[error("{0}")]
    Cut(#[from] CutError),
    /// An output name is unusable.
    #[error("{0}")]
    Name(#[from] NameError),
    /// The finished folder could not reach the library.
    #[error("{0}")]
    Transfer(#[from] TransferError),
    /// Plex could not be asked.
    #[error("{0}")]
    Plex(#[from] PlexError),
    /// The stream has no program, which holds its naming.
    #[error("stream {0} has no program, run program first")]
    NoProgram(String),
    /// The stream has no video format.
    #[error("stream {0} has no video format")]
    NoVideo(String),
    /// Fragments a cut needs have left the DVR.
    #[error("{0} fragments of this range have left the DVR")]
    LeftDvr(usize),
    /// A range did not arrive whole.
    #[error("{0} fragments of this range could not be fetched")]
    RangeIncomplete(usize),
    /// A clip of this name exists with other boundaries, and is not replaced.
    #[error("a clip named {0} already exists with other boundaries")]
    NameTaken(String),
    /// No clip has this ID.
    #[error("there is no clip {0}")]
    NoClip(i64),
    /// A finished file failed its checks, so it goes no further.
    #[error("the finished file failed verification: {0}")]
    Verification(String),
    /// A background task ended abnormally.
    #[error("a background task stopped unexpectedly")]
    Task,
}

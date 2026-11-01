//! Use cases and policies that tie the core, I/O and store together.

pub mod acquisition;
pub use dalsegno_core::analysis;
pub mod backfill;
pub mod config;
pub mod doctor;
pub mod error;
pub mod export;
pub mod family;
pub mod features;
pub mod fetch;
pub mod follow;
pub mod lifecycle;
pub mod monitor;
pub mod notify;
pub mod offline;
pub mod probe;
pub mod retention;
pub mod session;

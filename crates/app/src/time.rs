//! The wall clock.

use std::time::{SystemTime, UNIX_EPOCH};

/// Returns the time in epoch milliseconds.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

//! Loop analysis over in-memory series. All pure functions, usable offline (FR-N11).

mod diff;
mod ncc;
mod period;
mod series;
#[cfg(test)]
mod synthetic;
mod tracking;

pub use diff::{
    Diff, DiffConfig, KnownClip, Region, RegionClass, SelfCheck, VersionCycle, compare, locate_clip,
};
pub use ncc::{Correlator, Match, search};
pub use period::{PeriodConfig, PeriodEstimate, estimate_period};
pub use series::{Prepared, Series};
pub use tracking::{
    ChangeBounds, LagRun, RunKind, Signal, TrackConfig, TrackPoint, detect_signals, lag_bounds,
    narrow_change, summarise, track,
};

//! What an export is called and carries. All pure.

mod assembly;
mod boundary;
mod info;
mod naming;

pub use assembly::{CutError, FragmentRange, Piece, check_air_time, plan_cut};
pub use boundary::{
    EndCandidate, EndEdge, Evidence, Interval, Keyframe, RefinedPeriod, StartCandidate,
    end_candidates, parse_black, parse_silence, refine_period, start_candidates,
};
pub use info::sanitise_info;
pub use naming::{
    DEFAULT_SUFFIX, DEFAULT_TEMPLATE, ExportKind, ExportName, JstTime, NameError, NameParts,
    check_output_dir, render_name,
};

//! Cutting from the CLI: candidates, assembly, transfer and Plex (`docs/design/export.md`).

mod candidates;
mod cut;
mod publish;
#[cfg(test)]
mod tests;

pub use candidates::{CandidateReport, RefineReport};
pub use cut::{AirTime, CutReport, CutRequest, TagEdits};
pub use dalsegno_core::export::{DEFAULT_SUFFIX, DEFAULT_TEMPLATE, ExportKind};
pub use dalsegno_core::redact::Redacted;
pub use dalsegno_io::transfer::Ownership;
pub use dalsegno_store::programs::NewProgram;
pub use publish::{PlexTarget, PublishReport, PublishRequest};

use crate::acquisition::{Acquisition, Library};
use crate::error::AcquireError;
use crate::fetch::{FetchRange, MAX_RANGE_FRAGMENTS, fetch_range, lock};
use crate::lifecycle::PrefetchTarget;
use dalsegno_core::export::check_output_dir;
use dalsegno_io::ffmpeg::Ffmpeg;
use dalsegno_io::tools::find_program;
use dalsegno_store::retention::{HoldOwner, NewHold};
use dalsegno_store::streams::StreamRow;
use std::sync::Arc;

/// Finds ffmpeg and ffprobe.
pub fn find_tools() -> Result<Ffmpeg, AcquireError> {
    let find = |name: &str| {
        find_program(name).ok_or_else(|| {
            dalsegno_io::tools::ToolError::NotRun(name.to_string(), "not found".to_string())
        })
    };
    Ok(Ffmpeg {
        ffmpeg: find("ffmpeg")?,
        ffprobe: find("ffprobe")?,
    })
}

impl Library {
    /// Records a stream's program, which holds its naming and output folder.
    pub fn set_program(&self, stream: &str, program: &NewProgram) -> Result<i64, AcquireError> {
        let row = self.registered(stream)?;
        check_output_dir(&program.output_dir)?;
        let database = lock(&self.database)?;
        let id = database.insert_program(program)?;
        database.link_stream_to_program(row.id, id)?;
        Ok(id)
    }
}

impl Acquisition {
    /// Holds and fetches the video around a change (FR-E1). The hold comes first, and a
    /// repeat of the job only adds a second one, so it is safe to run again.
    pub async fn prefetch(
        &self,
        library: &Library,
        target: &PrefetchTarget,
    ) -> Result<(), AcquireError> {
        let row = library.registered(&target.stream)?;
        let itag = row
            .stream
            .video_itag
            .ok_or_else(|| AcquireError::NoVideo(row.stream.video_id.clone()))?;
        lock(&library.database)?.insert_hold(&NewHold {
            stream_id: row.id,
            itag,
            sq_first: target.first,
            sq_last: target.last,
            owner: HoldOwner::Change(target.change),
        })?;
        self.ensure_range(library, &row, itag, target.first, target.last)
            .await
    }

    /// Fetches whatever a range lacks on one track, and fails if it left the DVR (FR-E13).
    pub(crate) async fn ensure_range(
        &self,
        library: &Library,
        row: &StreamRow,
        itag: u32,
        first: u64,
        last: u64,
    ) -> Result<(), AcquireError> {
        let is_complete = {
            let database = lock(&library.database)?;
            let stored = database.fragments_in_range(row.id, itag, first, last)?;
            stored.len() as u64 == last - first + 1
        };
        if is_complete {
            return Ok(());
        }
        let fetcher = Arc::new(self.fetcher(library, row, itag)?);
        let mut start = first;
        while start <= last {
            let end = last.min(start + MAX_RANGE_FRAGMENTS - 1);
            let range = FetchRange {
                stream_id: row.id,
                itag,
                first: start,
                last: end,
            };
            let report = fetch_range(Arc::clone(&fetcher), range).await?;
            if !report.lost.is_empty() {
                return Err(AcquireError::LeftDvr(report.lost.len()));
            }
            if !report.failed.is_empty() {
                return Err(AcquireError::RangeIncomplete(report.failed.len()));
            }
            start = end + 1;
        }
        Ok(())
    }
}

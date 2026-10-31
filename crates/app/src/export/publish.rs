//! Moving a finished folder into the library and asking Plex for it (FR-E10, FR-E11).

use crate::acquisition::Library;
use crate::error::AcquireError;
use crate::fetch::lock;
use dalsegno_core::export::check_output_dir;
use dalsegno_core::redact::Redacted;
use dalsegno_io::plex::Plex;
use dalsegno_io::transfer::{Ownership, Transfer, TransferOutcome, transfer};
use dalsegno_store::clips::{ClipRow, PlexStatus};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::task::spawn_blocking;

/// Polls while Plex scans, for about two minutes.
const PLEX_POLLS: u32 = 24;
const PLEX_POLL_INTERVAL: Duration = Duration::from_secs(5);
/// Holds last this long past the owner's close (`docs/design/data-model.md`).
const HOLD_AFTER_CLOSE_MS: i64 = 7 * 24 * 3600 * 1000;

/// A Plex section to tell about a new folder.
#[derive(Debug, Clone)]
pub struct PlexTarget {
    /// Base URL.
    pub url: String,
    /// Token.
    pub token: Redacted<String>,
    /// Library section.
    pub section: u32,
}

/// One folder to publish.
#[derive(Debug, Clone)]
pub struct PublishRequest {
    /// Clip row ID.
    pub clip: i64,
    /// Library root.
    pub library_root: PathBuf,
    /// Owner and mode of what arrives.
    pub ownership: Ownership,
    /// Plex, when it is to be told.
    pub plex: Option<PlexTarget>,
}

/// What publishing did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublishReport {
    /// The folder in the library.
    pub target: PathBuf,
    /// Whether a retry found it already there.
    pub already_there: bool,
    /// Plex's answer, if it was asked: `confirmed` or `failed`.
    pub plex: Option<String>,
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// Returns the finished folder in the work area, once a retry has not removed it.
fn work_folder(library: &Library, clip: &ClipRow) -> PathBuf {
    library
        .work
        .join(format!("clip-{}", clip.id))
        .join(&clip.clip.name)
}

async fn move_folder(
    source: PathBuf,
    library_root: &Path,
    target: &Path,
    ownership: Ownership,
) -> Result<bool, AcquireError> {
    if !source.exists() && target.exists() {
        return Ok(true);
    }
    let job = Transfer {
        source,
        library: library_root.to_path_buf(),
        target: target.to_path_buf(),
        ownership,
    };
    let outcome = spawn_blocking(move || transfer(&job))
        .await
        .map_err(|_| AcquireError::Task)??;
    Ok(outcome == TransferOutcome::AlreadyThere)
}

async fn confirm(
    plex: &Plex,
    section: u32,
    relative: &str,
    interval: Duration,
    polls: u32,
) -> Result<bool, AcquireError> {
    plex.refresh(section).await?;
    for attempt in 0..polls {
        if plex.has_file(section, relative).await? {
            return Ok(true);
        }
        if attempt + 1 < polls {
            tokio::time::sleep(interval).await;
        }
    }
    Ok(false)
}

impl Library {
    /// Moves a finished folder into the library, tells Plex, and clears the work area.
    pub async fn publish(&self, request: &PublishRequest) -> Result<PublishReport, AcquireError> {
        self.publish_with(request, PLEX_POLL_INTERVAL, PLEX_POLLS)
            .await
    }

    pub(crate) async fn publish_with(
        &self,
        request: &PublishRequest,
        interval: Duration,
        polls: u32,
    ) -> Result<PublishReport, AcquireError> {
        let (clip, program) = {
            let database = lock(&self.database)?;
            let clip = database
                .clip(request.clip)?
                .ok_or(AcquireError::NoClip(request.clip))?;
            let program = database
                .program_of_stream(clip.clip.stream_id)?
                .ok_or_else(|| AcquireError::NoProgram(clip.clip.name.clone()))?;
            (clip, program)
        };
        check_output_dir(&program.program.output_dir)?;
        let name = &clip.clip.name;
        let target = request
            .library_root
            .join(&program.program.output_dir)
            .join(name);
        let source = work_folder(self, &clip);
        let already_there =
            move_folder(source, &request.library_root, &target, request.ownership).await?;
        {
            let database = lock(&self.database)?;
            database.set_clip_output(clip.id, &target.to_string_lossy())?;
            database.expire_clip_holds(clip.id, now_ms() + HOLD_AFTER_CLOSE_MS)?;
        }
        let directory = self.work.join(format!("clip-{}", clip.id));
        if directory.exists() {
            fs::remove_dir_all(directory)?;
        }
        let Some(plex) = &request.plex else {
            return Ok(PublishReport {
                target,
                already_there,
                plex: None,
            });
        };
        lock(&self.database)?.set_clip_plex_status(clip.id, PlexStatus::Requested)?;
        let client = Plex::new(&plex.url, plex.token.clone());
        let relative = format!("{name}/{name}.mp4");
        let is_listed = confirm(&client, plex.section, &relative, interval, polls).await;
        let status = match is_listed {
            Ok(true) => PlexStatus::Confirmed,
            Ok(false) | Err(_) => PlexStatus::Failed,
        };
        lock(&self.database)?.set_clip_plex_status(clip.id, status)?;
        is_listed?;
        Ok(PublishReport {
            target,
            already_there,
            plex: Some(
                match status {
                    PlexStatus::Confirmed => "confirmed",
                    _ => "failed",
                }
                .to_string(),
            ),
        })
    }
}

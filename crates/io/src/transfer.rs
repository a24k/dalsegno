//! Moving a finished folder into the library, whole or not at all (FR-E10).
//!
//! Plex reads embedded metadata only when an item first appears, and can link a stray file to a
//! stale item, so the folder is built under a dot-directory and renamed into place.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, chown};
use std::path::{Component, Path, PathBuf};

/// Dot-directories are ignored by Plex, and this one is on the library's file system.
const TEMPORARY_DIRECTORY: &str = ".dalsegno-tmp";

/// Who owns the files, and who may read them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ownership {
    /// Mode of files, such as `0o640`.
    pub mode: u32,
    /// User and group, when they must be set.
    pub owner: Option<(u32, u32)>,
}

/// One folder to move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transfer {
    /// Finished folder in the work area, holding only files.
    pub source: PathBuf,
    /// Library root.
    pub library: PathBuf,
    /// Target folder, inside the library.
    pub target: PathBuf,
    /// Applied to the folder and its files.
    pub ownership: Ownership,
}

/// How a transfer ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferOutcome {
    /// The folder was moved.
    Moved,
    /// A retry found it there already, and it verified.
    AlreadyThere,
}

/// Why a transfer failed.
#[derive(Debug, thiserror::Error)]
pub enum TransferError {
    /// The file system refused.
    #[error("{0}")]
    Io(#[from] io::Error),
    /// The target lies outside the library.
    #[error("the target {0} is outside the library")]
    OutsideLibrary(PathBuf),
    /// Something else is at the target, and it is never overwritten.
    #[error("{0} already exists and differs from this export")]
    TargetDiffers(PathBuf),
    /// A copied file has the wrong size.
    #[error("{0} did not copy whole")]
    ShortCopy(PathBuf),
}

/// Moves a folder into the library. Running it again after any failure is safe.
pub fn transfer(job: &Transfer) -> Result<TransferOutcome, TransferError> {
    let parent = job
        .target
        .parent()
        .ok_or_else(|| TransferError::OutsideLibrary(job.target.clone()))?;
    let library = fs::canonicalize(&job.library)?;
    let name = job
        .target
        .file_name()
        .ok_or_else(|| TransferError::OutsideLibrary(job.target.clone()))?;
    if !fs::canonicalize(nearest_existing(parent))?.starts_with(&library)
        || parent.components().any(|part| part == Component::ParentDir)
    {
        return Err(TransferError::OutsideLibrary(job.target.clone()));
    }
    fs::create_dir_all(parent)?;
    let wanted = sizes(&job.source)?;
    if fs::symlink_metadata(&job.target).is_ok() {
        return if sizes(&job.target).is_ok_and(|found| found == wanted) {
            Ok(TransferOutcome::AlreadyThere)
        } else {
            Err(TransferError::TargetDiffers(job.target.clone()))
        };
    }
    let temporary = library.join(TEMPORARY_DIRECTORY).join(name);
    if temporary.exists() {
        fs::remove_dir_all(&temporary)?;
    }
    fs::create_dir_all(&temporary)?;
    for file in wanted.keys() {
        fs::copy(job.source.join(file), temporary.join(file))?;
        fs::File::open(temporary.join(file))?.sync_all()?;
    }
    let copied = sizes(&temporary)?;
    if let Some(file) = wanted
        .iter()
        .find(|(file, size)| copied.get(*file) != Some(size))
    {
        return Err(TransferError::ShortCopy(file.0.clone()));
    }
    apply_to_files(&temporary, &wanted, job.ownership)?;
    fs::rename(&temporary, &job.target)?;
    // Moving a folder needs write access to it, so its own mode comes last.
    apply(
        &job.target,
        directory_mode(job.ownership.mode),
        job.ownership,
    )?;
    Ok(TransferOutcome::Moved)
}

/// Returns the deepest part of `path` that exists, so it can be checked before anything is made.
fn nearest_existing(path: &Path) -> &Path {
    path.ancestors()
        .find(|ancestor| ancestor.exists())
        .unwrap_or(path)
}

/// Maps each file of a folder to its size, and refuses anything that is not a plain file.
fn sizes(folder: &Path) -> io::Result<BTreeMap<PathBuf, u64>> {
    fs::read_dir(folder)?
        .map(|entry| {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            if !metadata.is_file() {
                return Err(io::Error::other(
                    "the folder holds something other than files",
                ));
            }
            Ok((PathBuf::from(entry.file_name()), metadata.len()))
        })
        .collect()
}

fn apply_to_files(
    folder: &Path,
    files: &BTreeMap<PathBuf, u64>,
    ownership: Ownership,
) -> io::Result<()> {
    files
        .keys()
        .try_for_each(|file| apply(&folder.join(file), ownership.mode, ownership))
}

fn apply(path: &Path, mode: u32, ownership: Ownership) -> io::Result<()> {
    if let Some((user, group)) = ownership.owner {
        chown(path, Some(user), Some(group))?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

/// Whoever may read a file must be able to enter its folder.
fn directory_mode(mode: u32) -> u32 {
    mode | ((mode & 0o444) >> 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::{TempDir, tempdir};

    const OWNERSHIP: Ownership = Ownership {
        mode: 0o640,
        owner: None,
    };

    struct Fixture {
        _root: TempDir,
        job: Transfer,
    }

    fn fixture() -> Fixture {
        let root = tempdir().unwrap();
        let source = root.path().join("work/Example");
        let library = root.path().join("library");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&library).unwrap();
        fs::write(source.join("Example.mp4"), b"video").unwrap();
        fs::write(source.join("Example.jpg"), b"poster").unwrap();
        let target = library.join("program/Example");
        Fixture {
            _root: root,
            job: Transfer {
                source,
                library,
                target,
                ownership: OWNERSHIP,
            },
        }
    }

    #[test]
    fn a_folder_arrives_whole() {
        let Fixture { job, _root } = fixture();

        let outcome = transfer(&job).unwrap();

        assert_eq!(outcome, TransferOutcome::Moved);
        assert_eq!(fs::read(job.target.join("Example.mp4")).unwrap(), b"video");
        assert_eq!(fs::read(job.target.join("Example.jpg")).unwrap(), b"poster");
    }

    #[test]
    fn nothing_is_left_in_the_temporary_directory() {
        let Fixture { job, _root } = fixture();

        transfer(&job).unwrap();

        let left = job.library.join(TEMPORARY_DIRECTORY).join("Example");
        assert!(!left.exists());
    }

    #[test]
    fn the_mode_is_applied() {
        let Fixture { job, _root } = fixture();

        transfer(&job).unwrap();

        let mode = |path: PathBuf| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(job.target.join("Example.mp4")), 0o640);
        assert_eq!(mode(job.target.clone()), 0o750);
    }

    #[test]
    fn a_retry_of_a_finished_transfer_is_done() {
        let Fixture { job, _root } = fixture();
        transfer(&job).unwrap();

        assert_eq!(transfer(&job).unwrap(), TransferOutcome::AlreadyThere);
    }

    #[test]
    fn a_leftover_temporary_folder_is_replaced() {
        let Fixture { job, _root } = fixture();
        let stale = job.library.join(TEMPORARY_DIRECTORY).join("Example");
        fs::create_dir_all(&stale).unwrap();
        fs::write(stale.join("stale.mp4"), b"half").unwrap();

        transfer(&job).unwrap();

        assert!(!job.target.join("stale.mp4").exists());
    }

    #[test]
    fn a_different_target_is_never_overwritten() {
        let Fixture { job, _root } = fixture();
        fs::create_dir_all(&job.target).unwrap();
        fs::write(job.target.join("Example.mp4"), b"something else").unwrap();

        assert!(matches!(
            transfer(&job),
            Err(TransferError::TargetDiffers(_))
        ));
        assert_eq!(
            fs::read(job.target.join("Example.mp4")).unwrap(),
            b"something else"
        );
    }

    #[test]
    fn a_target_outside_the_library_is_refused() {
        let Fixture { job, _root } = fixture();
        let outside = Transfer {
            target: job.library.join("../elsewhere/Example"),
            ..job
        };

        assert!(matches!(
            transfer(&outside),
            Err(TransferError::OutsideLibrary(_))
        ));
    }

    #[test]
    fn a_symlink_cannot_lead_out_of_the_library() {
        let Fixture { job, _root } = fixture();
        let elsewhere = job.library.parent().unwrap().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, job.library.join("program")).unwrap();

        assert!(matches!(
            transfer(&job),
            Err(TransferError::OutsideLibrary(_))
        ));
    }

    #[test]
    fn nothing_is_created_outside_the_library() {
        let Fixture { job, _root } = fixture();
        let elsewhere = job.library.parent().unwrap().join("elsewhere/deep/Example");
        let outside = Transfer {
            target: elsewhere.clone(),
            ..job
        };

        assert!(transfer(&outside).is_err());
        assert!(!elsewhere.parent().unwrap().parent().unwrap().exists());
    }

    #[test]
    fn a_read_only_mode_does_not_stop_the_folder_moving() {
        let Fixture { job, _root } = fixture();
        let strict = Transfer {
            ownership: Ownership {
                mode: 0o440,
                owner: None,
            },
            ..job
        };

        assert_eq!(transfer(&strict).unwrap(), TransferOutcome::Moved);
        let mode = fs::metadata(&strict.target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o550);
        fs::set_permissions(&strict.target, fs::Permissions::from_mode(0o750)).unwrap();
    }

    #[test]
    fn a_source_with_a_folder_inside_is_refused() {
        let Fixture { job, _root } = fixture();
        fs::create_dir_all(job.source.join("nested")).unwrap();

        assert!(matches!(transfer(&job), Err(TransferError::Io(_))));
    }

    #[test]
    fn a_folder_opens_for_whoever_may_read() {
        assert_eq!(directory_mode(0o640), 0o750);
        assert_eq!(directory_mode(0o644), 0o755);
        assert_eq!(directory_mode(0o600), 0o700);
    }
}

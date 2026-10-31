//! Fragment files on disk. Writes are atomic, so a stored fragment is always complete (FR-A7).

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

const FRAGMENTS_PER_DIRECTORY: u64 = 10_000;

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

/// Fragment storage under one data directory.
#[derive(Debug, Clone)]
pub struct FragmentStore {
    root: PathBuf,
}

impl FragmentStore {
    /// Uses `<data_dir>/fragments`.
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join("fragments"),
        }
    }

    fn path(&self, stream: i64, itag: u32, sq: u64) -> PathBuf {
        self.root
            .join(stream.to_string())
            .join(itag.to_string())
            .join((sq / FRAGMENTS_PER_DIRECTORY).to_string())
            .join(sq.to_string())
    }

    /// Returns whether a complete fragment is stored.
    pub fn contains(&self, stream: i64, itag: u32, sq: u64) -> bool {
        fs::metadata(self.path(stream, itag, sq)).is_ok_and(|metadata| metadata.len() > 0)
    }

    /// Writes to a temporary name, syncs, then renames.
    pub fn write(&self, stream: i64, itag: u32, sq: u64, bytes: &[u8]) -> io::Result<()> {
        let path = self.path(stream, itag, sq);
        let directory = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        fs::create_dir_all(directory)?;
        let temporary = unique_temporary(&path);
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;
        File::open(directory)?.sync_all()
    }

    /// Removes a fragment's file. An absent file is not an error.
    pub fn remove(&self, stream: i64, itag: u32, sq: u64) -> io::Result<()> {
        match fs::remove_file(self.path(stream, itag, sq)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    /// Lists every file under the store. Names that are not fragments have no key.
    pub fn files(&self) -> io::Result<Vec<StoredFile>> {
        let mut files = Vec::new();
        for stream in subdirectories(&self.root)? {
            for track in subdirectories(&stream)? {
                for bucket in subdirectories(&track)? {
                    for entry in fs::read_dir(&bucket)? {
                        let path = entry?.path();
                        files.extend(StoredFile::describe(&stream, &track, path)?);
                    }
                }
            }
        }
        Ok(files)
    }

    /// Reads one stored fragment.
    pub fn read(&self, stream: i64, itag: u32, sq: u64) -> io::Result<Vec<u8>> {
        fs::read(self.path(stream, itag, sq))
    }

    /// Joins `first..=last` in `sq` order into `output`, which appears only when complete.
    pub fn concat(
        &self,
        stream: i64,
        itag: u32,
        first: u64,
        last: u64,
        output: &Path,
    ) -> io::Result<()> {
        let temporary = with_suffix(output, ".part");
        let joined = self.join_into(stream, itag, first, last, &temporary);
        joined
            .and_then(|()| fs::rename(&temporary, output))
            .inspect_err(|_| {
                let _ = fs::remove_file(&temporary);
            })
    }

    fn join_into(
        &self,
        stream: i64,
        itag: u32,
        first: u64,
        last: u64,
        target: &Path,
    ) -> io::Result<()> {
        let mut writer = BufWriter::new(File::create(target)?);
        for sq in first..=last {
            io::copy(&mut File::open(self.path(stream, itag, sq))?, &mut writer)?;
        }
        writer.into_inner()?.sync_all()
    }
}

/// One file found under the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    /// Where it is.
    pub path: PathBuf,
    /// Stream, format and `sq`, when the name is a fragment's.
    pub key: Option<(i64, u32, u64)>,
    /// Whether the name is a temporary one.
    pub is_temporary: bool,
    /// Last change.
    pub modified: SystemTime,
}

impl StoredFile {
    /// Returns `None` for a file that vanished, such as a temporary one just renamed.
    fn describe(stream: &Path, track: &Path, path: PathBuf) -> io::Result<Option<Self>> {
        let key = number(stream)
            .zip(number(track))
            .zip(number(&path))
            .map(|((stream, itag), sq)| (stream, itag, sq));
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(Some(Self {
            is_temporary: path.extension().is_some_and(|extension| extension == "tmp"),
            modified: metadata.modified()?,
            key,
            path,
        }))
    }
}

fn number<T: FromStr>(path: &Path) -> Option<T> {
    path.file_name()?.to_str()?.parse().ok()
}

fn subdirectories(path: &Path) -> io::Result<Vec<PathBuf>> {
    match fs::read_dir(path) {
        Ok(entries) => entries
            .map(|entry| entry.map(|entry| entry.path()))
            .filter(|path| path.as_ref().map_or(true, |path| path.is_dir()))
            .collect(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

// A fixed name would let two writers truncate each other's file just before the rename.
fn unique_temporary(path: &Path) -> PathBuf {
    let count = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    with_suffix(path, &format!(".{}.{count}.tmp", process::id()))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: i64 = 3;

    fn stream() -> i64 {
        STREAM
    }

    #[test]
    fn splits_directories_by_ten_thousand() {
        let store = FragmentStore::new(Path::new("/data"));

        assert_eq!(
            store.path(stream(), 140, 2_453_470),
            Path::new("/data/fragments/3/140/245/2453470")
        );
    }

    #[test]
    fn written_fragment_is_contained_and_readable() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());

        store.write(stream(), 140, 7, b"payload").unwrap();

        assert!(store.contains(stream(), 140, 7));
        assert_eq!(store.read(stream(), 140, 7).unwrap(), b"payload");
    }

    #[test]
    fn absent_and_empty_fragments_are_not_contained() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());

        store.write(stream(), 140, 7, b"").unwrap();

        assert!(!store.contains(stream(), 140, 7));
        assert!(!store.contains(stream(), 140, 8));
    }

    #[test]
    fn removing_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());
        store.write(stream(), 140, 7, b"payload").unwrap();

        store.remove(stream(), 140, 7).unwrap();
        store.remove(stream(), 140, 7).unwrap();

        assert!(!store.contains(stream(), 140, 7));
    }

    #[test]
    fn files_lists_fragments_and_other_names() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());
        store.write(stream(), 140, 7, b"payload").unwrap();
        let path = store.path(stream(), 140, 7);
        fs::write(unique_temporary(&path), b"par").unwrap();

        let mut files = store.files().unwrap();
        files.sort_by_key(|file| file.key.is_none());

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].key, Some((STREAM, 140, 7)));
        assert!(!files[0].is_temporary);
        assert_eq!(files[1].key, None);
        assert!(files[1].is_temporary);
    }

    #[test]
    fn an_empty_store_has_no_files() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(FragmentStore::new(dir.path()).files().unwrap(), []);
    }

    #[test]
    fn writing_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());

        store.write(stream(), 140, 7, b"payload").unwrap();

        let parent = store.path(stream(), 140, 7);
        let entries = fs::read_dir(parent.parent().unwrap()).unwrap().count();
        assert_eq!(entries, 1);
    }

    #[test]
    fn interrupted_write_is_replaced_on_retry() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());
        store.write(stream(), 140, 6, b"other").unwrap();
        let path = store.path(stream(), 140, 7);
        fs::write(path.with_extension("tmp"), b"par").unwrap();

        assert!(!store.contains(stream(), 140, 7));
        store.write(stream(), 140, 7, b"payload").unwrap();

        assert_eq!(store.read(stream(), 140, 7).unwrap(), b"payload");
    }

    #[test]
    fn concat_joins_in_order_across_directories() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());
        store.write(stream(), 140, 9_999, b"a").unwrap();
        store.write(stream(), 140, 10_000, b"b").unwrap();
        store.write(stream(), 140, 10_001, b"c").unwrap();
        let output = dir.path().join("out");

        store.concat(stream(), 140, 9_999, 10_001, &output).unwrap();

        assert_eq!(fs::read(output).unwrap(), b"abc");
    }

    #[test]
    fn concurrent_writers_never_publish_a_partial_fragment() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());
        let payload = vec![7u8; 256 * 1024];

        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..20 {
                        store.write(stream(), 140, 7, &payload).unwrap();
                    }
                });
            }
        });

        assert_eq!(store.read(stream(), 140, 7).unwrap(), payload);
    }

    #[test]
    fn concat_keeps_the_extension_of_a_neighbouring_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());
        store.write(stream(), 140, 1, b"a").unwrap();
        let neighbour = dir.path().join("out.part");
        fs::write(&neighbour, b"keep").unwrap();

        store
            .concat(stream(), 140, 1, 1, &dir.path().join("out.mp4"))
            .unwrap();

        assert_eq!(fs::read(neighbour).unwrap(), b"keep");
    }

    #[test]
    fn failed_concat_removes_its_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());
        let output = dir.path().join("out");

        store.concat(stream(), 140, 1, 1, &output).unwrap_err();

        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            0,
            "no partial output may remain"
        );
    }

    #[test]
    fn concat_with_a_missing_fragment_leaves_no_output() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());
        store.write(stream(), 140, 1, b"a").unwrap();
        let output = dir.path().join("out");

        let error = store.concat(stream(), 140, 1, 2, &output).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!output.exists());
    }
}

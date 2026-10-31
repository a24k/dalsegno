//! Fragment files on disk. Writes are atomic, so a stored fragment is always complete (FR-A7).

use dalsegno_core::stream::VideoId;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

const FRAGMENTS_PER_DIRECTORY: u64 = 10_000;

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

    fn path(&self, stream: &VideoId, itag: u32, sq: u64) -> PathBuf {
        self.root
            .join(stream.as_str())
            .join(itag.to_string())
            .join((sq / FRAGMENTS_PER_DIRECTORY).to_string())
            .join(sq.to_string())
    }

    /// Returns whether a complete fragment is stored.
    pub fn contains(&self, stream: &VideoId, itag: u32, sq: u64) -> bool {
        fs::metadata(self.path(stream, itag, sq)).is_ok_and(|metadata| metadata.len() > 0)
    }

    /// Writes to a temporary name, syncs, then renames.
    pub fn write(&self, stream: &VideoId, itag: u32, sq: u64, bytes: &[u8]) -> io::Result<()> {
        let path = self.path(stream, itag, sq);
        let directory = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
        fs::create_dir_all(directory)?;
        let temporary = path.with_extension("tmp");
        let mut file = File::create(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;
        File::open(directory)?.sync_all()
    }

    /// Reads one stored fragment.
    pub fn read(&self, stream: &VideoId, itag: u32, sq: u64) -> io::Result<Vec<u8>> {
        fs::read(self.path(stream, itag, sq))
    }

    /// Joins `first..=last` in `sq` order into `output`, which appears only when complete.
    pub fn concat(
        &self,
        stream: &VideoId,
        itag: u32,
        first: u64,
        last: u64,
        output: &Path,
    ) -> io::Result<()> {
        let temporary = output.with_extension("part");
        let mut writer = BufWriter::new(File::create(&temporary)?);
        for sq in first..=last {
            io::copy(&mut File::open(self.path(stream, itag, sq))?, &mut writer)?;
        }
        writer.into_inner()?.sync_all()?;
        fs::rename(temporary, output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream() -> VideoId {
        VideoId::parse("abcDEF123_-").unwrap()
    }

    #[test]
    fn splits_directories_by_ten_thousand() {
        let store = FragmentStore::new(Path::new("/data"));

        assert_eq!(
            store.path(&stream(), 140, 2_453_470),
            Path::new("/data/fragments/abcDEF123_-/140/245/2453470")
        );
    }

    #[test]
    fn written_fragment_is_contained_and_readable() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());

        store.write(&stream(), 140, 7, b"payload").unwrap();

        assert!(store.contains(&stream(), 140, 7));
        assert_eq!(store.read(&stream(), 140, 7).unwrap(), b"payload");
    }

    #[test]
    fn absent_and_empty_fragments_are_not_contained() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());

        store.write(&stream(), 140, 7, b"").unwrap();

        assert!(!store.contains(&stream(), 140, 7));
        assert!(!store.contains(&stream(), 140, 8));
    }

    #[test]
    fn writing_leaves_no_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());

        store.write(&stream(), 140, 7, b"payload").unwrap();

        let parent = store.path(&stream(), 140, 7);
        let entries = fs::read_dir(parent.parent().unwrap()).unwrap().count();
        assert_eq!(entries, 1);
    }

    #[test]
    fn interrupted_write_is_replaced_on_retry() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());
        store.write(&stream(), 140, 6, b"other").unwrap();
        let path = store.path(&stream(), 140, 7);
        fs::write(path.with_extension("tmp"), b"par").unwrap();

        assert!(!store.contains(&stream(), 140, 7));
        store.write(&stream(), 140, 7, b"payload").unwrap();

        assert_eq!(store.read(&stream(), 140, 7).unwrap(), b"payload");
    }

    #[test]
    fn concat_joins_in_order_across_directories() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());
        store.write(&stream(), 140, 9_999, b"a").unwrap();
        store.write(&stream(), 140, 10_000, b"b").unwrap();
        store.write(&stream(), 140, 10_001, b"c").unwrap();
        let output = dir.path().join("out");

        store
            .concat(&stream(), 140, 9_999, 10_001, &output)
            .unwrap();

        assert_eq!(fs::read(output).unwrap(), b"abc");
    }

    #[test]
    fn concat_with_a_missing_fragment_leaves_no_output() {
        let dir = tempfile::tempdir().unwrap();
        let store = FragmentStore::new(dir.path());
        store.write(&stream(), 140, 1, b"a").unwrap();
        let output = dir.path().join("out");

        let error = store.concat(&stream(), 140, 1, 2, &output).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!output.exists());
    }
}

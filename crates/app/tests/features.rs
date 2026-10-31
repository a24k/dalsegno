//! Extraction, gaps and the sweep, using real fragments when fixtures are available.

use dalsegno_app::features::extract_features;
use dalsegno_app::retention::sweep;
use dalsegno_core::container::parse_mp4_fragment;
use dalsegno_io::fragments::FragmentStore;
use dalsegno_store::database::Database;
use dalsegno_store::features::EnvelopeRow;
use dalsegno_store::fragments::FragmentRow;
use dalsegno_store::streams::{NewStream, StreamRow};
use dalsegno_test_support::require_fixtures;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

const AUDIO: u32 = 140;
const VIDEO: u32 = 248;
const FIRST_SOURCE_SQ: u64 = 2_528_515;
const DAY_MS: i64 = 24 * 3600 * 1000;
const NOW: i64 = 100 * DAY_MS;
const TOLERANCE: f32 = 1e-5;

struct Library {
    database: Database,
    store: FragmentStore,
    stream: StreamRow,
    _data: TempDir,
}

impl Library {
    fn new() -> Self {
        let data = tempfile::tempdir().unwrap();
        let database = Database::open_in_memory().unwrap();
        let stream = NewStream {
            video_id: "abcDEF123_-".to_string(),
            title: "Example TV".to_string(),
            channel_id: None,
            channel_name: None,
            thumbnail_url: None,
            video_itag: Some(VIDEO),
            audio_itag: AUDIO,
            fragment_len: 5.0,
            registered_at: 0,
        };
        let id = database.insert_stream(&stream).unwrap();
        Self {
            store: FragmentStore::new(data.path()),
            stream: StreamRow { id, stream },
            database,
            _data: data,
        }
    }

    fn add_bytes(&self, itag: u32, sq: u64, bytes: &[u8], fetched_at: i64) {
        self.store.write(self.stream.id, itag, sq, bytes).unwrap();
        let row = FragmentRow {
            stream_id: self.stream.id,
            itag,
            sq,
            stream_time: 0.0,
            duration: 5.0,
            size: bytes.len() as u64,
            fetched_at,
        };
        self.database.put_fragment(&row).unwrap();
    }

    fn add_source(&self, source: &Path, sq: u64, source_offset: u64) {
        let name = (FIRST_SOURCE_SQ + source_offset).to_string();
        self.add_bytes(AUDIO, sq, &fs::read(source.join(name)).unwrap(), NOW);
    }

    fn add_range(&self, source: &Path, offsets: std::ops::Range<u64>) {
        for offset in offsets {
            self.add_source(source, 100 + offset, offset);
        }
    }

    fn extract(&mut self) -> dalsegno_app::features::FeatureReport {
        extract_features(&mut self.database, &self.store, &self.stream).unwrap()
    }

    fn rows(&self) -> Vec<EnvelopeRow> {
        self.database
            .envelope_rows(self.stream.id, 0, 1_000_000)
            .unwrap()
    }

    fn run_bounds(&self) -> Vec<(u64, u64)> {
        self.database
            .feature_runs(self.stream.id)
            .unwrap()
            .iter()
            .map(|run| (run.sq_first, run.next_sq))
            .collect()
    }
}

fn flatten(rows: &[EnvelopeRow]) -> Vec<(u64, f32)> {
    rows.iter()
        .flat_map(|row| {
            row.values
                .iter()
                .enumerate()
                .map(|(index, value)| (row.first_bin + index as u64, *value))
        })
        .collect()
}

fn assert_close(a: &[(u64, f32)], b: &[(u64, f32)]) {
    assert_eq!(a.len(), b.len());
    for (left, right) in a.iter().zip(b) {
        assert_eq!(left.0, right.0);
        assert!((left.1 - right.1).abs() <= TOLERANCE, "{left:?} {right:?}");
    }
}

fn batch(source: &Path, offsets: std::ops::Range<u64>) -> Vec<(u64, f32)> {
    let mut library = Library::new();
    library.add_range(source, offsets);
    library.extract();
    flatten(&library.rows())
}

#[test]
fn a_stream_with_no_audio_does_nothing() {
    let mut library = Library::new();

    let report = library.extract();

    assert_eq!(report.fragments, 0);
    assert_eq!(library.run_bounds(), []);
}

#[test]
fn an_unreadable_fragment_is_reported_and_not_retried_in_the_pass() {
    let mut library = Library::new();
    library.add_bytes(AUDIO, 5, b"garbage", NOW);

    let report = library.extract();

    assert_eq!(report.unreadable, [5]);
    assert_eq!(library.rows(), []);
}

#[test]
fn a_missing_file_is_reported() {
    let mut library = Library::new();
    library.add_bytes(AUDIO, 5, b"garbage", NOW);
    library.store.remove(library.stream.id, AUDIO, 5).unwrap();

    assert_eq!(library.extract().unreadable, [5]);
}

#[test]
fn one_pass_makes_one_run_with_contiguous_bins() {
    let fixtures = require_fixtures!();
    let source = fixtures.join("frag-140");
    let mut library = Library::new();
    library.add_range(&source, 0..12);

    let report = library.extract();

    assert_eq!(report.fragments, 12);
    assert!(report.bins > 500);
    assert_eq!(report.unreadable, Vec::<u64>::new());
    assert_eq!(library.run_bounds(), [(100, 112)]);
    let bins = flatten(&library.rows());
    assert!(bins.windows(2).all(|pair| pair[1].0 == pair[0].0 + 1));
}

#[test]
fn a_rerun_with_nothing_new_changes_nothing() {
    let fixtures = require_fixtures!();
    let mut library = Library::new();
    library.add_range(&fixtures.join("frag-140"), 0..6);
    library.extract();
    let before = library.rows();

    let report = library.extract();

    assert_eq!(report.fragments, 0);
    assert_eq!(library.rows(), before);
}

#[test]
fn incremental_equals_batch_across_a_restart() {
    let fixtures = require_fixtures!();
    let source = fixtures.join("frag-140");
    let expected = batch(&source, 0..24);
    let mut library = Library::new();

    for (first, last) in [(0, 7), (7, 8), (8, 24)] {
        library.add_range(&source, first..last);
        library.extract();
    }

    assert_eq!(library.run_bounds(), [(100, 124)]);
    assert_close(&flatten(&library.rows()), &expected);
}

#[test]
fn an_open_gap_does_not_block_later_audio() {
    let fixtures = require_fixtures!();
    let source = fixtures.join("frag-140");
    let mut library = Library::new();
    library.add_range(&source, 0..6);
    library.add_range(&source, 10..16);

    library.extract();

    assert_eq!(library.run_bounds(), [(100, 106), (110, 116)]);
}

#[test]
fn a_filled_gap_merges_runs_and_equals_batch() {
    let fixtures = require_fixtures!();
    let source = fixtures.join("frag-140");
    let expected = batch(&source, 0..16);
    let mut library = Library::new();
    library.add_range(&source, 0..6);
    library.add_range(&source, 10..16);
    library.extract();

    library.add_range(&source, 6..10);
    library.extract();

    assert_eq!(library.run_bounds(), [(100, 116)]);
    assert_close(&flatten(&library.rows()), &expected);
}

#[test]
fn a_jump_in_stream_time_starts_a_new_run() {
    let fixtures = require_fixtures!();
    let source = fixtures.join("frag-140");
    let mut library = Library::new();
    library.add_range(&source, 0..4);
    for sq in 104..108 {
        library.add_source(&source, sq, sq - 100 + 40);
    }

    library.extract();

    assert_eq!(library.run_bounds(), [(100, 104), (104, 108)]);
    let again = library.extract();
    assert_eq!(again.fragments, 0);
    assert_eq!(library.run_bounds(), [(100, 104), (104, 108)]);
}

#[test]
fn stream_time_of_a_run_comes_from_the_fragment() {
    let fixtures = require_fixtures!();
    let source = fixtures.join("frag-140");
    let mut library = Library::new();
    library.add_range(&source, 0..2);
    library.extract();
    let bytes = fs::read(source.join(FIRST_SOURCE_SQ.to_string())).unwrap();
    let expected = parse_mp4_fragment(&bytes).unwrap().start_time();

    let runs = library.database.feature_runs(library.stream.id).unwrap();

    assert_eq!(runs[0].anchor_time, expected);
    assert_eq!(library.rows()[0].first_bin, (expected * 10.0).ceil() as u64);
}

#[test]
fn sweep_removes_expired_fragments_with_their_files() {
    let mut library = Library::new();
    library.add_bytes(AUDIO, 1, b"old", NOW - 15 * DAY_MS);
    library.add_bytes(AUDIO, 2, b"new", NOW - 13 * DAY_MS);
    library.add_bytes(VIDEO, 2, b"unheld", NOW);

    let report = sweep(&mut library.database, &library.store, NOW).unwrap();

    assert_eq!(report.expired, 2);
    assert!(!library.store.contains(library.stream.id, AUDIO, 1));
    assert!(library.store.contains(library.stream.id, AUDIO, 2));
    assert!(!library.store.contains(library.stream.id, VIDEO, 2));
}

#[test]
fn sweep_repairs_rows_without_files_and_files_without_rows() {
    let mut library = Library::new();
    library.add_bytes(AUDIO, 1, b"kept", NOW);
    library.add_bytes(AUDIO, 2, b"file gone", NOW);
    library.store.remove(library.stream.id, AUDIO, 2).unwrap();
    library
        .store
        .write(library.stream.id, AUDIO, 3, b"orphan")
        .unwrap();

    let report = sweep(&mut library.database, &library.store, NOW).unwrap();

    assert_eq!((report.repaired, report.orphans), (1, 1));
    assert!(library.store.contains(library.stream.id, AUDIO, 1));
    assert!(!library.store.contains(library.stream.id, AUDIO, 3));
    let rows = library
        .database
        .fragments_in_range(library.stream.id, AUDIO, 0, 10)
        .unwrap();
    assert_eq!(rows.len(), 1);
}

//! A cut and a publish through real ffmpeg. Everything is generated, so no fixtures are needed,
//! and the tests return early where ffmpeg is not installed. Nothing here can reach the origin,
//! as fragments are stored up front and the step that fetches them is left out.

use super::candidates::candidates_with;
use super::cut::cut_with;
use super::*;
use dalsegno_core::container::read_timing;
use dalsegno_core::export::{Evidence, Interval, parse_black, refine_period};
use dalsegno_store::clips::PlexStatus;
use dalsegno_store::fragments::FragmentRow;
use dalsegno_store::streams::NewStream;
use dalsegno_test_support::require_fixtures;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::{TempDir, tempdir};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

const STREAM: &str = "AAAAAAAAAAA";
const START_SQ: u64 = 100;
const STREAM_TIME: f64 = 1_000.0;
const SECONDS: f64 = 8.0;
const NAMED_AT: i64 = 1_793_372_160;
const NAME: &str = "Example TV 20261030-2356 追加分 (2026)";
const VIDEO_ITAG: u32 = 248;
const AUDIO_ITAG: u32 = 140;

struct Setup {
    root: TempDir,
    library: Library,
    tools: Ffmpeg,
}

async fn cut_stored(setup: &Setup, request: &CutRequest) -> Result<CutReport, AcquireError> {
    let ensure = async |_: &StreamRow, _, _, _| Ok(());
    cut_with(&setup.library, &setup.tools, request, ensure).await
}

fn generate(ffmpeg: &Path, args: &[&str], output: &Path) -> Vec<u8> {
    let status = Command::new(ffmpeg)
        .args(["-v", "error", "-y"])
        .args(args)
        .arg(output)
        .status()
        .unwrap();
    assert!(status.success());
    fs::read(output).unwrap()
}

fn store_fragment(library: &Library, id: i64, itag: u32, bytes: &[u8]) {
    library.store.write(id, itag, START_SQ, bytes).unwrap();
    lock(&library.database)
        .unwrap()
        .put_fragment(&FragmentRow {
            stream_id: id,
            itag,
            sq: START_SQ,
            stream_time: STREAM_TIME,
            duration: SECONDS,
            size: bytes.len() as u64,
            fetched_at: 0,
        })
        .unwrap();
}

/// Registers a stream with one long fragment of each track.
fn setup() -> Option<Setup> {
    let tools = find_tools().ok()?;
    let root = tempdir().unwrap();
    let library = Library::open(&root.path().join("data")).unwrap();
    let id = lock(&library.database)
        .unwrap()
        .insert_stream(&NewStream {
            video_id: STREAM.to_string(),
            title: "Example TV -An Archive-".to_string(),
            channel_id: Some("UCexample".to_string()),
            channel_name: Some("The Channel".to_string()),
            thumbnail_url: None,
            video_itag: Some(VIDEO_ITAG),
            audio_itag: AUDIO_ITAG,
            fragment_len: SECONDS,
            registered_at: 0,
        })
        .unwrap();
    library
        .set_program(
            STREAM,
            &NewProgram {
                short_name: "Example TV".to_string(),
                channel_id: None,
                name_template: DEFAULT_TEMPLATE.to_string(),
                addition_suffix: DEFAULT_SUFFIX.to_string(),
                artist: None,
                content_date: Some("2026-01-01".to_string()),
                description: None,
                output_dir: "Example TV".to_string(),
            },
        )
        .unwrap();
    let video = generate(
        &tools.ffmpeg,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc=s=64x64:r=10:d=8",
            "-c:v",
            "mpeg4",
        ],
        &root.path().join("v.mkv"),
    );
    let audio = generate(
        &tools.ffmpeg,
        &["-f", "lavfi", "-i", "sine=f=440:d=8", "-c:a", "aac"],
        &root.path().join("a.m4a"),
    );
    store_fragment(&library, id, VIDEO_ITAG, &video);
    store_fragment(&library, id, AUDIO_ITAG, &audio);
    Some(Setup {
        root,
        library,
        tools,
    })
}

fn request() -> CutRequest {
    CutRequest {
        stream: STREAM.to_string(),
        kind: ExportKind::Addition,
        start_sq: START_SQ,
        duration: 6.0,
        named_at: NAMED_AT,
        air_time: None,
        override_air_time: false,
        tags: TagEdits::default(),
        poster_seconds: Some(1.0),
    }
}

async fn plex(found: bool) -> MockServer {
    let server = MockServer::start().await;
    let files = if found {
        json!([{"Media": [{"Part": [{"file": format!("/media/{NAME}/{NAME}.mp4")}]}]}])
    } else {
        json!([])
    };
    Mock::given(method("GET"))
        .and(path_regex("/refresh$"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path_regex("/all$"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"MediaContainer": {"Metadata": files}})),
        )
        .mount(&server)
        .await;
    server
}

fn publishing(setup: &Setup, clip: i64, plex: Option<&MockServer>) -> PublishRequest {
    let library_root = setup.root.path().join("library");
    fs::create_dir_all(&library_root).unwrap();
    PublishRequest {
        clip,
        library_root,
        ownership: Ownership {
            mode: 0o640,
            owner: None,
        },
        plex: plex.map(|server| PlexTarget {
            url: server.uri(),
            token: Redacted::new("token".to_string()),
            section: 3,
        }),
    }
}

#[tokio::test]
async fn a_cut_leaves_a_complete_folder() {
    let Some(setup) = setup() else {
        return;
    };

    let report = cut_stored(&setup, &request()).await.unwrap();

    assert_eq!(report.name, NAME);
    assert!((report.duration - 6.0).abs() < 0.11);
    let file = |suffix: &str| report.folder.join(format!("{NAME}{suffix}"));
    for suffix in [".mp4", ".jpg", "-fanart.jpg", ".info.json"] {
        assert!(file(suffix).exists(), "{suffix}");
    }
    assert_eq!(
        fs::read(file(".jpg")).unwrap(),
        fs::read(file("-fanart.jpg")).unwrap()
    );
    let names: Vec<_> = fs::read_dir(&report.folder).unwrap().collect();
    assert_eq!(names.len(), 4);
}

#[tokio::test]
async fn a_cut_records_how_to_make_it_again() {
    let Some(setup) = setup() else {
        return;
    };

    let report = cut_stored(&setup, &request()).await.unwrap();

    let database = lock(&setup.library.database).unwrap();
    let clip = database.clip(report.clip_id).unwrap().unwrap();
    assert_eq!(
        (clip.clip.sq_first, clip.clip.sq_last),
        (START_SQ, START_SQ)
    );
    assert_eq!(clip.clip.duration, 6.0);
    assert!(clip.clip.metadata.contains("Only the part added"));
    assert!(clip.clip.metadata.contains(&NAMED_AT.to_string()));
}

#[tokio::test]
async fn cutting_again_reuses_the_clip() {
    let Some(setup) = setup() else {
        return;
    };
    let first = cut_stored(&setup, &request()).await.unwrap();
    let second = cut_stored(&setup, &request()).await.unwrap();

    assert_eq!(first.clip_id, second.clip_id);
}

#[tokio::test]
async fn a_name_is_not_reused_for_other_boundaries() {
    let Some(setup) = setup() else {
        return;
    };
    cut_stored(&setup, &request()).await.unwrap();
    let other = CutRequest {
        duration: 5.0,
        ..request()
    };

    let result = cut_stored(&setup, &other).await;

    assert!(matches!(result, Err(AcquireError::NameTaken(_))));
}

#[tokio::test]
async fn a_cycle_without_an_air_time_is_blocked() {
    let Some(setup) = setup() else {
        return;
    };
    let cycle = CutRequest {
        kind: ExportKind::Cycle,
        ..request()
    };

    let result = cut_stored(&setup, &cycle).await;

    assert!(matches!(result, Err(AcquireError::InvalidRequest(_))));
}

#[tokio::test]
async fn a_stream_with_no_program_cannot_be_cut() {
    let Some(setup) = setup() else {
        return;
    };
    lock(&setup.library.database)
        .unwrap()
        .connection()
        .execute("UPDATE streams SET program_id = NULL", [])
        .unwrap();

    let result = cut_stored(&setup, &request()).await;

    assert!(matches!(result, Err(AcquireError::NoProgram(_))));
}

#[tokio::test]
async fn a_cut_near_the_end_needs_only_its_own_fragments() {
    let Some(setup) = setup() else {
        return;
    };
    let asked = std::sync::Mutex::new(Vec::new());
    let ensure = async |_: &StreamRow, _, first, last| {
        asked.lock().unwrap().push((first, last));
        Ok(())
    };

    cut_with(&setup.library, &setup.tools, &request(), ensure)
        .await
        .unwrap();

    let largest = asked.lock().unwrap().iter().map(|r| r.1).max().unwrap();
    assert_eq!(largest, START_SQ, "6 s fits in one 8-second fragment");
}

#[tokio::test]
async fn a_folder_that_leaves_the_library_is_not_recorded() {
    let Some(setup) = setup() else {
        return;
    };
    let program = NewProgram {
        short_name: "Example".to_string(),
        channel_id: None,
        name_template: DEFAULT_TEMPLATE.to_string(),
        addition_suffix: DEFAULT_SUFFIX.to_string(),
        artist: None,
        content_date: None,
        description: None,
        output_dir: "../elsewhere".to_string(),
    };

    let result = setup.library.set_program(STREAM, &program);

    assert!(matches!(result, Err(AcquireError::Name(_))));
}

#[tokio::test]
async fn a_clip_is_held_on_both_tracks() {
    let Some(setup) = setup() else {
        return;
    };

    let report = cut_stored(&setup, &request()).await.unwrap();

    let database = lock(&setup.library.database).unwrap();
    let holds: i64 = database
        .connection()
        .query_row(
            "SELECT COUNT(*) FROM retention_holds WHERE clip_id = ?1",
            [report.clip_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(holds, 2);
}

#[tokio::test]
async fn publishing_moves_the_folder_and_plex_confirms_it() {
    let Some(setup) = setup() else {
        return;
    };
    let report = cut_stored(&setup, &request()).await.unwrap();
    let server = plex(true).await;

    let published = setup
        .library
        .publish_with(
            &publishing(&setup, report.clip_id, Some(&server)),
            std::time::Duration::ZERO,
            1,
        )
        .await
        .unwrap();

    assert_eq!(published.plex.as_deref(), Some("confirmed"));
    assert!(!published.already_there);
    assert!(published.target.join(format!("{NAME}.mp4")).exists());
    assert!(!report.folder.exists());
    let database = lock(&setup.library.database).unwrap();
    let clip = database.clip(report.clip_id).unwrap().unwrap();
    assert_eq!(clip.plex_status, PlexStatus::Confirmed);
    assert_eq!(
        clip.output_path.as_deref(),
        Some(published.target.to_string_lossy().as_ref())
    );
}

#[tokio::test]
async fn publishing_twice_is_safe() {
    let Some(setup) = setup() else {
        return;
    };
    let report = cut_stored(&setup, &request()).await.unwrap();
    let job = publishing(&setup, report.clip_id, None);

    setup.library.publish(&job).await.unwrap();
    let again = setup.library.publish(&job).await.unwrap();

    assert!(again.already_there);
}

#[tokio::test]
async fn plex_not_listing_the_file_is_reported_as_failed() {
    let Some(setup) = setup() else {
        return;
    };
    let report = cut_stored(&setup, &request()).await.unwrap();
    let server = plex(false).await;

    let published = setup
        .library
        .publish_with(
            &publishing(&setup, report.clip_id, Some(&server)),
            std::time::Duration::ZERO,
            2,
        )
        .await
        .unwrap();

    assert_eq!(published.plex.as_deref(), Some("failed"));
    let database = lock(&setup.library.database).unwrap();
    let clip = database.clip(report.clip_id).unwrap().unwrap();
    assert_eq!(clip.plex_status, PlexStatus::Failed);
}

#[tokio::test]
async fn publishing_an_unknown_clip_fails() {
    let Some(setup) = setup() else {
        return;
    };

    let result = setup.library.publish(&publishing(&setup, 99, None)).await;

    assert!(matches!(result, Err(AcquireError::NoClip(99))));
}

/// The start of the addition the operator cut by hand, and its length.
const HAND_CUT_START_SQ: u64 = 2_558_035;
const HAND_CUT_LAST_SQ: u64 = 2_558_889;
const HAND_CUT_SECONDS: f64 = 4_131.85;
const HAND_CUT_DATETIME: &str = "20261030-2356";
/// Measured on the hand cut against its fragments: the last packet was already cut short.
const MAX_AUDIO_PACKETS_APART: usize = 1;

/// Returns each packet's checksum, which ignores its container.
fn packet_hashes(ffmpeg: &Path, file: &Path, stream: &str) -> Vec<String> {
    let output = Command::new(ffmpeg)
        .args(["-v", "error", "-i"])
        .arg(file)
        .args(["-map", stream, "-c", "copy", "-f", "framemd5", "-"])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.rsplit(',').next())
        .map(|hash| hash.trim().to_string())
        .collect()
}

fn find_hand_cut(fixtures: &Path) -> PathBuf {
    let is_it = |name: &str| name.contains(HAND_CUT_DATETIME);
    let folder = fs::read_dir(fixtures)
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(HAND_CUT_DATETIME)
        })
        .expect("hand cut")
        .path();
    fs::read_dir(folder)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension().is_some_and(|ext| ext == "mp4") && is_it(&path.to_string_lossy())
        })
        .expect("hand cut mp4")
}

// This is the exit of the milestone: the tool makes what the manual process made.
#[tokio::test]
async fn a_cut_from_real_fragments_equals_the_cut_made_by_hand() {
    require_fixtures!();
    let fixtures = dalsegno_test_support::fixtures_dir().unwrap();
    let Some(setup) = setup() else {
        return;
    };
    lock(&setup.library.database)
        .unwrap()
        .connection()
        .execute("UPDATE streams SET fragment_len = 5.0", [])
        .unwrap();
    for itag in [VIDEO_ITAG, AUDIO_ITAG] {
        store_range(
            &setup.library,
            &fixtures,
            itag,
            HAND_CUT_START_SQ,
            HAND_CUT_LAST_SQ,
        );
    }
    let cut = CutRequest {
        start_sq: HAND_CUT_START_SQ,
        duration: HAND_CUT_SECONDS,
        ..request()
    };

    let report = cut_stored(&setup, &cut).await.unwrap();

    let ours = report.folder.join(format!("{NAME}.mp4"));
    let theirs = find_hand_cut(&fixtures);
    let hashes = |file: &Path, stream: &str| packet_hashes(&setup.tools.ffmpeg, file, stream);
    let (our_video, their_video) = (hashes(&ours, "0:v"), hashes(&theirs, "0:v"));
    let first_difference = our_video.iter().zip(&their_video).position(|(a, b)| a != b);
    assert_eq!(
        (first_difference, our_video.len()),
        (None, their_video.len()),
        "packets of the video"
    );
    let (our_audio, their_audio) = (hashes(&ours, "0:a"), hashes(&theirs, "0:a"));
    let same = our_audio
        .iter()
        .zip(&their_audio)
        .filter(|(a, b)| a == b)
        .count();
    assert!(
        their_audio.len() - same <= MAX_AUDIO_PACKETS_APART,
        "{same} of {} and {}",
        their_audio.len(),
        our_audio.len()
    );
}

/// The first fragment of the saved 10/24 version.
const VERSION_START_SQ: u64 = 2_453_470;
/// Fragments from there to the black that opens the worked example, and one cycle on.
const FIRST_BLACK_OFFSET: u64 = 33;
const LATER_BLACK_OFFSET: u64 = 7_563;
const BLACK_REACH: u64 = 4;
const AUDIO_PERIOD: f64 = 37_648.7;
const VIDEO_PERIOD: f64 = 37_649.067;

async fn black_near(setup: &Setup, fixtures: &Path, centre: u64) -> Vec<Interval> {
    let first = centre - BLACK_REACH;
    let joined = setup.root.path().join(format!("black-{centre}.webm"));
    let bytes: Vec<u8> = (first..=centre + BLACK_REACH)
        .flat_map(|sq| fs::read(fixtures.join("frag-248").join(sq.to_string())).unwrap())
        .collect();
    fs::write(&joined, bytes).unwrap();
    let start = read_timing(&fs::read(fixtures.join("frag-248").join(first.to_string())).unwrap())
        .unwrap()
        .start_time;
    parse_black(&setup.tools.detect_black(&joined).await.unwrap())
        .iter()
        .map(|interval| Interval {
            start: interval.start + start,
            end: interval.end + start,
        })
        .collect()
}

#[tokio::test]
async fn the_period_from_real_black_matches_the_measured_one() {
    require_fixtures!();
    let fixtures = dalsegno_test_support::fixtures_dir().unwrap();
    let Some(setup) = setup() else {
        return;
    };

    let first = black_near(&setup, &fixtures, VERSION_START_SQ + FIRST_BLACK_OFFSET).await;
    let later = black_near(&setup, &fixtures, VERSION_START_SQ + LATER_BLACK_OFFSET).await;

    println!("first {first:?}\nlater {later:?}");
    let refined = refine_period(&first, &later, AUDIO_PERIOD).expect("agreeing black");
    println!("{refined:?}");
    assert!((refined.period - VIDEO_PERIOD).abs() < 0.02, "{refined:?}");
    assert!(refined.agreeing >= 2, "{refined:?}");
}

/// A boundary of the saved 10/24 version, where black and silence fall together.
const CANDIDATE_CENTRE_SQ: u64 = VERSION_START_SQ + FIRST_BLACK_OFFSET;
const CANDIDATE_REACH: u64 = 4;

fn store_range(library: &Library, fixtures: &Path, itag: u32, first: u64, last: u64) {
    for sq in first..=last {
        let bytes = fs::read(fixtures.join(format!("frag-{itag}")).join(sq.to_string())).unwrap();
        let timing = read_timing(&bytes).unwrap();
        library.store.write(1, itag, sq, &bytes).unwrap();
        lock(&library.database)
            .unwrap()
            .put_fragment(&FragmentRow {
                stream_id: 1,
                itag,
                sq,
                stream_time: timing.start_time,
                duration: timing.duration.unwrap_or(5.0),
                size: bytes.len() as u64,
                fetched_at: 0,
            })
            .unwrap();
    }
}

#[tokio::test]
async fn candidates_from_real_fragments_rank_and_offer_keyframes() {
    require_fixtures!();
    let fixtures = dalsegno_test_support::fixtures_dir().unwrap();
    let Some(setup) = setup() else {
        return;
    };
    lock(&setup.library.database)
        .unwrap()
        .connection()
        .execute("UPDATE streams SET fragment_len = 5.0", [])
        .unwrap();
    let (first, last) = (
        CANDIDATE_CENTRE_SQ - CANDIDATE_REACH,
        CANDIDATE_CENTRE_SQ + CANDIDATE_REACH,
    );
    store_range(&setup.library, &fixtures, VIDEO_ITAG, first, last);
    store_range(&setup.library, &fixtures, AUDIO_ITAG, first, last);
    let time = lock(&setup.library.database)
        .unwrap()
        .fragments_in_range(1, AUDIO_ITAG, CANDIDATE_CENTRE_SQ, CANDIDATE_CENTRE_SQ)
        .unwrap()[0]
        .stream_time;
    let ensure = async |_: &StreamRow, _, _, _| Ok(());

    let report = candidates_with(&setup.library, &setup.tools, STREAM, time, ensure)
        .await
        .unwrap();

    println!("{report:#?}");
    assert!(!report.starts.is_empty());
    assert!(!report.ends.is_empty());
    assert!(report.contact_sheet.exists());
    let frames: Vec<u64> = report.starts.iter().map(|c| c.keyframe.sq).collect();
    assert!(frames.iter().all(|sq| (first..=last).contains(sq)));
    assert_eq!(report.starts[0].evidence, Evidence::Both);
}

//! Comparison on synthetic cycles.

use super::super::synthetic::{content, prepared};
use super::*;

const P: f64 = 3000.0;

fn version(data: &Prepared, id: i64, period: f64) -> VersionCycle<'_> {
    VersionCycle {
        id,
        data,
        start: 0.0,
        period,
    }
}

fn classes(diff: &Diff) -> Vec<RegionClass> {
    diff.regions.iter().map(|region| region.class).collect()
}

fn region(diff: &Diff, class: RegionClass) -> Region {
    *diff
        .regions
        .iter()
        .find(|region| region.class == class)
        .unwrap()
}

fn compare_cycles(old: &[f32], new: &[f32]) -> Diff {
    let previous = prepared(&[old, old].concat());
    let current = prepared(&[new, new].concat());
    let period = new.len() as f64 / 10.0;
    let versions = [version(&previous, 1, old.len() as f64 / 10.0)];
    compare(
        &current,
        0.0,
        period,
        &versions,
        &[],
        &DiffConfig::default(),
    )
}

#[test]
fn an_identical_cycle_is_all_existing() {
    let cycle = content(1, P);

    let diff = compare_cycles(&cycle, &cycle);

    assert_eq!(classes(&diff), [RegionClass::Existing]);
    assert!(
        diff.checks.iter().all(|check| check.passed),
        "{:?}",
        diff.checks
    );
}

#[test]
fn an_insertion_is_new_and_passes_the_checks() {
    let old = content(2, P);
    let insert = content(3, 500.0);
    let new = [&old[..12_000], &insert, &old[12_000..]].concat();

    let diff = compare_cycles(&old, &new);

    let added = region(&diff, RegionClass::New);
    assert!((added.start - 1200.0).abs() < 60.0, "{added:?}");
    assert!((added.end - added.start - 500.0).abs() < 60.0, "{added:?}");
    assert_eq!(diff.shift_changes.len(), 1);
    assert!((diff.shift_changes[0] - 500.0).abs() < 0.2);
    assert!(
        diff.checks.iter().all(|check| check.passed),
        "{:?}",
        diff.checks
    );
}

#[test]
fn edges_land_near_the_truth_from_either_side() {
    let old = content(40, P);
    let insert = content(41, 400.0);

    for at in [10_000, 10_123, 10_257, 10_300] {
        let new = [&old[..at], &insert, &old[at..]].concat();

        let diff = compare_cycles(&old, &new);

        let added = region(&diff, RegionClass::New);
        let truth = at as f64 / 10.0;
        assert!((added.start - truth).abs() <= 3.0, "{at}: {added:?}");
        assert!((added.end - truth - 400.0).abs() <= 3.0, "{at}: {added:?}");
    }
}

#[test]
fn a_replacement_reports_what_was_deleted() {
    let old = content(4, P);
    let insert = content(5, 900.0);
    let new = [&old[..12_000], &insert, &old[16_000..]].concat();

    let diff = compare_cycles(&old, &new);

    let removed = region(&diff, RegionClass::Deleted);
    assert!(
        (removed.end - removed.start - 400.0).abs() < 60.0,
        "{removed:?}"
    );
    assert!((diff.shift_changes[0] - 500.0).abs() < 0.2);
    assert!(
        diff.checks.iter().all(|check| check.passed),
        "{:?}",
        diff.checks
    );
}

#[test]
fn a_track_from_an_older_version_is_a_revival() {
    let older = content(6, 2000.0);
    let old = content(7, P);
    let new = [&old[..12_000], &older[500..5500], &old[12_000..]].concat();
    let previous = prepared(&[&old[..], &old[..]].concat());
    let oldest = prepared(&[&older[..], &older[..]].concat());
    let current = prepared(&[&new[..], &new[..]].concat());
    let versions = [version(&previous, 2, P), version(&oldest, 1, 2000.0)];

    let diff = compare(
        &current,
        0.0,
        P + 500.0,
        &versions,
        &[],
        &DiffConfig::default(),
    );

    let revived = region(&diff, RegionClass::Revival);
    assert_eq!(revived.version, Some(1));
    assert!(
        !classes(&diff).contains(&RegionClass::New),
        "{:?}",
        diff.regions
    );
}

#[test]
fn a_net_zero_replacement_of_a_song_is_new() {
    let old = content(50, P);
    let song = content(51, 211.0);
    let new = [&old[..12_000], &song, &old[14_110..]].concat();

    let diff = compare_cycles(&old, &new);

    let added = region(&diff, RegionClass::New);
    assert!((added.end - added.start - 211.0).abs() < 10.0, "{added:?}");
    assert!(classes(&diff).contains(&RegionClass::Deleted));
}

/// A cycle of songs apart from two flat videos, whose level alone says they are the same audio.
fn cycle_with_middle(middle: &[f32], noise: u64) -> Vec<f32> {
    let gap = vec![0.0; 20];
    let flat = |seed: u64| -> Vec<f32> {
        content(seed, 600.0)
            .iter()
            .map(|value| 9.0 + (value - 8.0) * 0.25)
            .collect()
    };
    [
        content(50, 1000.0).as_slice(),
        &gap,
        &flat(noise),
        &gap,
        middle,
        &gap,
        &flat(noise + 1),
        &gap,
        content(51, 1000.0).as_slice(),
    ]
    .concat()
}

#[test]
fn a_replaced_video_is_new_and_the_videos_around_it_are_not() {
    let old = cycle_with_middle(&content(52, 200.0), 60);
    let new = cycle_with_middle(&content(53, 200.0), 70);

    let diff = compare_cycles(&old, &new);

    let added = region(&diff, RegionClass::New);
    assert!((added.start - 1603.0).abs() < 5.0, "{:?}", diff.regions);
    assert!((added.end - 1805.0).abs() < 5.0, "{:?}", diff.regions);
    assert_eq!(
        classes(&diff)
            .iter()
            .filter(|class| **class == RegionClass::New)
            .count(),
        1
    );
}

#[test]
fn an_insertion_gives_back_the_videos_at_either_end() {
    let gap = vec![0.0; 20];
    let old = [content(55, 1000.0), gap.clone(), content(56, 1000.0)].concat();
    let new = [&old[..10_020], &content(57, 300.0), &gap, &old[10_000..]].concat();

    let diff = compare_cycles(&old, &new);

    let added = region(&diff, RegionClass::New);
    assert!((added.start - 1001.0).abs() < 5.0, "{:?}", diff.regions);
    assert!((added.end - 1303.0).abs() < 5.0, "{:?}", diff.regions);
}

#[test]
fn silence_is_no_data_and_not_new() {
    let old = content(52, P);
    let silence = vec![5.0; 3_000];
    let new = [&old[..12_000], &silence, &old[12_000..]].concat();

    let diff = compare_cycles(&old, &new);

    assert!(
        classes(&diff).contains(&RegionClass::NoData),
        "{:?}",
        diff.regions
    );
    assert!(
        !classes(&diff).contains(&RegionClass::New),
        "{:?}",
        diff.regions
    );
}

#[test]
fn a_silent_clip_is_not_located_in_silence() {
    let parts = [content(53, 300.0), vec![5.0; 3_000], content(54, 300.0)];
    let current = prepared(&parts.concat());
    let clip = prepared(&vec![5.0; 2_000]);

    assert_eq!(locate_clip(&current, &clip, 0.0..900.0, 0.8), []);
}

#[test]
fn a_copy_out_of_order_counts_as_inserted() {
    let old = content(20, P);
    let insert = content(21, 500.0);
    let copy = &old[2_000..6_000];
    let new = [&old[..20_000], copy, &insert, &old[20_000..]].concat();

    let diff = compare_cycles(&old, &new);

    assert!(
        diff.regions.iter().any(|region| region.moved),
        "{:?}",
        diff.regions
    );
    assert_eq!(diff.shift_changes.len(), 1, "{:?}", diff.shift_changes);
    assert!((diff.shift_changes[0] - 900.0).abs() < 0.2);
    assert!(
        diff.checks.iter().all(|check| check.passed),
        "{:?}",
        diff.checks
    );
}

#[test]
fn a_registered_clip_is_known_and_not_new() {
    let old = content(8, P);
    let clip = content(9, 211.0);
    let new = [&old[..12_000], &clip, &old[12_000..]].concat();
    let previous = prepared(&[&old[..], &old[..]].concat());
    let current = prepared(&[&new[..], &new[..]].concat());
    let versions = [version(&previous, 1, P)];
    let known = [KnownClip {
        start: 1200.0,
        end: 1411.0,
    }];

    let diff = compare(
        &current,
        0.0,
        P + 211.0,
        &versions,
        &known,
        &DiffConfig::default(),
    );

    assert!(
        classes(&diff).contains(&RegionClass::Known),
        "{:?}",
        diff.regions
    );
    assert!(!classes(&diff).contains(&RegionClass::New));
}

#[test]
fn a_brief_match_elsewhere_does_not_split_a_region() {
    let old = content(10, P);
    let clip = old[20_000..21_000].to_vec();
    let mut repeated = old.clone();
    repeated[5_000..6_000].copy_from_slice(&clip);

    let diff = compare_cycles(&repeated, &repeated);

    assert_eq!(
        classes(&diff),
        [RegionClass::Existing],
        "{:?}",
        diff.regions
    );
}

#[test]
fn a_position_past_the_stored_audio_is_found_one_period_back() {
    let data = prepared(&content(14, 1000.0));
    let config = DiffConfig::default();
    let comparison = EdgeComparison {
        current: &data,
        earlier: &data,
        period: 400.0,
        offset: -700.0,
        config: &config,
    };

    assert_eq!(comparison.earlier_time(200.0, 60.0), 900.0);
    assert_eq!(comparison.earlier_time(350.0, 60.0), 650.0);
}

#[test]
fn a_replacement_by_material_found_elsewhere_is_not_bridged() {
    let old = content(13, P);
    let clip = old[20_000..22_110].to_vec();
    let mut new = old.clone();
    new[5_000..7_110].copy_from_slice(&clip);

    let diff = compare_cycles(&old, &new);

    assert!(
        classes(&diff).contains(&RegionClass::New),
        "{:?}",
        diff.regions
    );
}

#[test]
fn a_failed_check_is_reported_and_not_fatal() {
    let old = content(11, P);
    let new = [&old[..], &old[..3000]].concat();
    let previous = prepared(&[&old[..], &old[..]].concat());
    let current = prepared(&[&new[..], &new[..]].concat());
    let versions = [version(&previous, 1, P + 7.0)];

    let diff = compare(&current, 0.0, P, &versions, &[], &DiffConfig::default());

    assert!(diff.checks.iter().any(|check| !check.passed));
}

#[test]
fn no_versions_makes_everything_new() {
    let cycle = content(12, 600.0);
    let current = prepared(&cycle);

    let diff = compare(&current, 0.0, 500.0, &[], &[], &DiffConfig::default());

    assert_eq!(classes(&diff), [RegionClass::New]);
}

#[test]
fn a_clip_is_located_at_each_place_it_plays() {
    let clip = content(30, 211.0);
    let parts = [
        content(31, 500.0),
        clip.clone(),
        content(32, 300.0),
        clip.clone(),
        content(33, 100.0),
    ];
    let current = prepared(&parts.concat());

    let found = locate_clip(&current, &prepared(&clip), 0.0..1322.0, 0.8);

    let starts: Vec<f64> = found.iter().map(|clip| clip.start).collect();
    assert_eq!(starts, [500.0, 1011.0]);
    assert_eq!(found[0].end, 711.0);
}

#[test]
fn a_clip_longer_than_the_series_is_not_found() {
    let current = prepared(&content(34, 10.0));
    let clip = prepared(&content(35, 20.0));

    assert_eq!(locate_clip(&current, &clip, 0.0..10.0, 0.8), []);
}

/// Flat audio, as the same recording reaches two versions: the level stays close, but correlation
/// stays too low to place it by searching or to move an edge.
fn flat_copies(seconds: f64) -> (Vec<f32>, Vec<f32>) {
    let shrink = |seed: u64, scale: f32| -> Vec<f32> {
        content(seed, seconds)
            .iter()
            .map(|value| (value - 8.0) * scale)
            .collect()
    };
    let shared = shrink(40, 0.25);
    let noise = shrink(41, 0.35);
    let old = shared.iter().map(|value| 8.0 + value).collect();
    let new = shared
        .iter()
        .zip(&noise)
        .map(|(a, b)| 8.0 + a + b)
        .collect();
    (old, new)
}

#[test]
fn flat_audio_after_an_addition_stays_existing_with_no_silence_to_mark_it() {
    let (before, after) = (content(42, 1200.0), content(43, 1500.0));
    let (flat_old, flat_new) = flat_copies(300.0);
    let added = content(44, 500.0);
    let old = [&before[..], &flat_old, &after].concat();
    let new = [&before[..], &added, &flat_new, &after].concat();

    let diff = compare_cycles(&old, &new);

    let inserted = region(&diff, RegionClass::New);
    assert!((inserted.end - 1700.0).abs() < 15.0, "{inserted:?}");
    assert!(
        !classes(&diff).contains(&RegionClass::Deleted),
        "{:?}",
        diff.regions
    );
}

#[test]
fn an_addition_keeps_its_end_when_the_audio_after_it_is_unrelated() {
    let (before, after) = (content(45, 1200.0), content(46, 1500.0));
    let (removed, added) = (content(47, 300.0), content(48, 800.0));
    let old = [&before[..], &removed, &after].concat();
    let new = [&before[..], &added, &after].concat();

    let diff = compare_cycles(&old, &new);

    assert!((region(&diff, RegionClass::New).end - 2000.0).abs() < 15.0);
    assert!(classes(&diff).contains(&RegionClass::Deleted));
}

fn silence(seconds: f64) -> Vec<f32> {
    vec![0.0; (seconds * 10.0) as usize]
}

/// Content with a silence every `every` seconds.
fn with_silences(seed: u64, every: f64, count: usize) -> Vec<f32> {
    (0..count)
        .flat_map(|index| [content(seed + index as u64, every), silence(1.0)].concat())
        .collect()
}

#[test]
fn silences_inside_new_audio_are_reported_as_boundaries() {
    let old = content(50, P);
    let added = [content(51, 300.0), silence(1.0), content(52, 300.0)].concat();
    let new = [&old[..12_000], &added, &old[12_000..]].concat();

    let diff = compare_cycles(&old, &new);

    assert_eq!(
        diff.video_boundaries.len(),
        1,
        "{:?}",
        diff.video_boundaries
    );
    assert!((diff.video_boundaries[0] - 1500.5).abs() < 1.0);
}

#[test]
fn the_edges_of_new_audio_are_not_boundaries_inside_it() {
    let old = [content(53, 1200.0), silence(1.0), content(54, 1799.0)].concat();
    let added = content(55, 500.0);
    let new = [&old[..12_005], &added, &old[12_005..]].concat();

    let diff = compare_cycles(&old, &new);

    assert_eq!(diff.video_boundaries, Vec::<f64>::new());
}

#[test]
fn silences_recurring_at_the_shift_pass_a_check_of_their_own() {
    let cycle = with_silences(60, 299.0, 10);

    let diff = compare_cycles(&cycle, &cycle);

    let check = diff
        .checks
        .iter()
        .find(|check| check.name == SILENCE_CHECK)
        .expect("silences to check");
    assert!(check.passed, "{check:?}");
    assert_eq!(check.actual, 1.0);
}

#[test]
fn silences_missing_from_the_old_cycle_fail_that_check() {
    let current = prepared(&with_silences(70, 299.0, 10));
    let previous = prepared(&content(71, P));
    let versions = [version(&previous, 1, P)];
    let regions = [Region {
        class: RegionClass::Existing,
        start: 0.0,
        end: 3000.0,
        shift: Some(0.0),
        version: Some(1),
        match_start: None,
        moved: false,
    }];

    let check = silences_recur(&regions, &current, &versions).expect("silences to check");

    assert!(!check.passed, "{check:?}");
    assert_eq!(check.actual, 0.0);
}

#[test]
fn a_silence_at_the_edge_of_new_audio_has_no_false_middle() {
    let current = prepared(&[content(72, 100.0), silence(10.0), content(73, 100.0)].concat());
    let added = Region {
        class: RegionClass::New,
        start: 0.0,
        end: 105.0,
        shift: None,
        version: None,
        match_start: None,
        moved: false,
    };

    assert_eq!(boundaries_inside_new(&[added], &current), Vec::<f64>::new());
}

#[test]
fn audio_with_no_silences_skips_that_check() {
    let cycle = content(80, P);

    let diff = compare_cycles(&cycle, &cycle);

    assert!(diff.checks.iter().all(|check| check.name != SILENCE_CHECK));
}

#[test]
fn distance_wraps_around_the_period() {
    assert_eq!(circular_distance(1.0, 2999.0, 3000.0), 2.0);
    assert_eq!(circular_distance(5.0, 8.0, 3000.0), 3.0);
}

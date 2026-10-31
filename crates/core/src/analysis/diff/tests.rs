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
fn a_brief_flip_does_not_split_a_region() {
    let found = |shift| {
        Window::Found(Found {
            version: 0,
            shift,
            position: 0.0,
            offset: 0.0,
        })
    };
    let previous = prepared(&content(10, 100.0));
    let versions = [version(&previous, 1, P)];
    let items = [
        found(100.0),
        found(100.0),
        found(100.0),
        found(900.0),
        found(100.0),
        found(100.0),
        found(100.0),
    ];
    let config = DiffConfig {
        min_windows: 3,
        ..DiffConfig::default()
    };

    let spans = consolidate(group(&items, &versions, &config), &versions, &config);

    assert_eq!(spans.len(), 1);
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

#[test]
fn distance_wraps_around_the_period() {
    assert_eq!(circular_distance(1.0, 2999.0, 3000.0), 2.0);
    assert_eq!(circular_distance(5.0, 8.0, 3000.0), 3.0);
}

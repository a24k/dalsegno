//! The two versions of a change side by side, each from the start of its cycle.

use crate::error::AcquireError;
use dalsegno_store::database::Database;
use dalsegno_store::status::RegionStatus;
use serde::Serialize;
use serde_json::{Value, json};

/// The old cycle ends this long before a change, as comparison searches 10 minutes past a
/// cycle's ends and must not find the new audio there. It loops, so any whole cycle will do.
const OLD_CYCLE_CLEARANCE_SECONDS: f64 = 660.0;

/// Where the old version's cycle starts for a change, in stream time.
pub fn old_cycle_start(change_start: f64, old_period: f64) -> f64 {
    change_start - old_period - OLD_CYCLE_CLEARANCE_SECONDS
}

/// One stretch of a bar.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BarRegion {
    /// Row ID of the stored region.
    pub id: i64,
    /// `existing`, `new`, `revival`, `known`, `deleted` or `no_data`.
    pub class: String,
    /// Start, in stream time.
    pub start: f64,
    /// End, in stream time.
    pub end: f64,
    /// Where it starts in the version it matched.
    pub match_start: Option<f64>,
    /// Video boundaries inside it, in stream time.
    pub video_boundaries: Vec<f64>,
}

/// One bar of the version strip.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bar {
    /// The version it shows.
    pub version_id: i64,
    /// Stream time of the start of its cycle.
    pub start: f64,
    /// Length of its cycle, in seconds.
    pub period: f64,
    /// What lies in it, by start.
    pub regions: Vec<BarRegion>,
}

/// Both versions of a change, each from the start of its cycle.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Comparison {
    /// The change.
    pub change_id: i64,
    /// `provisional`, `confirmed`, `resolved` or `dismissed`.
    pub state: String,
    /// The version that was on air.
    pub old: Option<Bar>,
    /// The version that replaced it, once the change is confirmed.
    pub new: Option<Bar>,
    /// The self-checks, as stored.
    pub checks: Value,
}

fn bar_region(region: &RegionStatus, start: f64, boundaries: &[f64]) -> BarRegion {
    let end = start + (region.end - region.start);
    BarRegion {
        id: region.id,
        class: region.class.clone(),
        start,
        end,
        match_start: region.match_start,
        video_boundaries: boundaries
            .iter()
            .copied()
            .filter(|boundary| *boundary > start && *boundary < end)
            .collect(),
    }
}

/// Places the old version's bar. Deleted regions are already in its time, and any other region
/// that matched it is projected into its cycle, which loops.
pub fn old_bar(version_id: i64, start: f64, period: f64, regions: &[RegionStatus]) -> Bar {
    let mut placed: Vec<BarRegion> = regions
        .iter()
        .filter_map(|region| {
            if region.class == "deleted" {
                return Some(bar_region(region, region.start, &[]));
            }
            let matched = region
                .match_start
                .filter(|_| region.match_version == Some(version_id))?;
            let projected = start + (matched - start).rem_euclid(period);
            Some(bar_region(region, projected, &[]))
        })
        .collect();
    placed.sort_by(|a, b| a.start.total_cmp(&b.start));
    Bar {
        version_id,
        start,
        period,
        regions: placed,
    }
}

/// Places the new version's bar from the regions that are not deleted, with the video
/// boundaries that fall inside each.
pub fn new_bar(
    version_id: i64,
    start: f64,
    period: f64,
    regions: &[RegionStatus],
    boundaries: &[f64],
) -> Bar {
    Bar {
        version_id,
        start,
        period,
        regions: regions
            .iter()
            .filter(|region| region.class != "deleted")
            .map(|region| bar_region(region, region.start, boundaries))
            .collect(),
    }
}

fn stored_boundaries(text: Option<&str>) -> Vec<f64> {
    text.and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or_default()
}

/// Reads a change's comparison, or none when the change does not exist.
pub fn comparison(database: &Database, change_id: i64) -> Result<Option<Comparison>, AcquireError> {
    let Some(change) = database.change_status(change_id)? else {
        return Ok(None);
    };
    let regions = database.change_regions(change_id)?;
    let old = match change.old_version_id {
        Some(id) => database.version(id)?.map(|version| {
            let start = old_cycle_start(change.start_time, version.period);
            old_bar(id, start, version.period, &regions)
        }),
        None => None,
    };
    let boundaries = stored_boundaries(change.boundaries.as_deref());
    let new = match change.new_version_id {
        Some(id) => database.version(id)?.map(|version| {
            new_bar(
                id,
                version.anchor_time,
                version.period,
                &regions,
                &boundaries,
            )
        }),
        None => None,
    };
    Ok(Some(Comparison {
        change_id,
        state: change.state,
        old,
        new,
        checks: change
            .checks
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_else(|| json!([])),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dalsegno_store::changes::{Confirmation, NewChange, NewRegion, NewVersion};
    use dalsegno_store::programs::NewProgram;
    use dalsegno_store::streams::NewStream;

    fn region(
        id: i64,
        class: &str,
        start: f64,
        end: f64,
        matched: Option<(i64, f64)>,
    ) -> RegionStatus {
        RegionStatus {
            id,
            class: class.to_string(),
            start,
            end,
            match_version: matched.map(|(version, _)| version),
            match_start: matched.map(|(_, start)| start),
        }
    }

    #[test]
    fn the_old_cycle_starts_a_period_and_a_clearance_before_the_change() {
        assert_eq!(old_cycle_start(10_000.0, 3_600.0), 5_740.0);
    }

    #[test]
    fn an_existing_region_is_projected_into_the_old_cycle() {
        let regions = [region(1, "existing", 0.0, 100.0, Some((7, 1_050.0)))];

        let bar = old_bar(7, 1_000.0, 500.0, &regions);

        assert_eq!(bar.regions[0].start, 1_050.0);
        assert_eq!(bar.regions[0].end, 1_150.0);
    }

    #[test]
    fn a_projection_wraps_around_the_cycle() {
        let regions = [
            region(1, "existing", 0.0, 100.0, Some((7, 1_700.0))),
            region(2, "existing", 100.0, 150.0, Some((7, 900.0))),
        ];

        let bar = old_bar(7, 1_000.0, 500.0, &regions);

        let starts: Vec<f64> = bar.regions.iter().map(|region| region.start).collect();
        assert_eq!(starts, [1_200.0, 1_400.0]);
        assert_eq!(bar.regions[0].id, 1);
    }

    #[test]
    fn a_deleted_region_keeps_its_old_time_and_others_from_elsewhere_are_left_out() {
        let regions = [
            region(1, "deleted", 1_300.0, 1_340.0, None),
            region(2, "new", 0.0, 50.0, None),
            region(3, "existing", 50.0, 90.0, Some((8, 1_000.0))),
        ];

        let bar = old_bar(7, 1_000.0, 500.0, &regions);

        assert_eq!(bar.regions.len(), 1);
        assert_eq!(
            (
                bar.regions[0].class.as_str(),
                bar.regions[0].start,
                bar.regions[0].end
            ),
            ("deleted", 1_300.0, 1_340.0)
        );
    }

    #[test]
    fn the_new_bar_leaves_out_deleted_regions_and_assigns_video_boundaries() {
        let regions = [
            region(1, "existing", 200.0, 300.0, Some((7, 0.0))),
            region(2, "deleted", 50.0, 80.0, None),
            region(3, "new", 300.0, 400.0, None),
        ];

        let bar = new_bar(9, 200.0, 200.0, &regions, &[250.0, 300.0, 350.0, 500.0]);

        let ids: Vec<i64> = bar.regions.iter().map(|region| region.id).collect();
        assert_eq!(ids, [1, 3]);
        assert_eq!(bar.regions[0].video_boundaries, [250.0]);
        assert_eq!(bar.regions[1].video_boundaries, [350.0]);
    }

    fn database_with_change() -> (Database, i64, i64, i64) {
        let database = Database::open_in_memory().unwrap();
        let stream = database
            .insert_stream(&NewStream {
                video_id: "abcDEF123_-".to_string(),
                title: "Example TV".to_string(),
                channel_id: None,
                channel_name: None,
                thumbnail_url: None,
                video_itag: Some(248),
                audio_itag: 140,
                fragment_len: 5.0,
                registered_at: 0,
            })
            .unwrap();
        let program = database
            .insert_program(&NewProgram {
                short_name: "Example TV".to_string(),
                channel_id: None,
                name_template: "{name}".to_string(),
                addition_suffix: "x".to_string(),
                artist: None,
                content_date: None,
                description: None,
                output_dir: "Example TV".to_string(),
            })
            .unwrap();
        let version = |anchor_time: f64| NewVersion {
            program_id: program,
            stream_id: stream,
            period: 1_000.0,
            anchor_time,
            seen_at: 0,
            on_air_from: 0.0,
        };
        let old = database.insert_version(&version(0.0)).unwrap();
        let new = database.insert_version(&version(9_000.0)).unwrap();
        let change = database
            .insert_change(&NewChange {
                program_id: program,
                stream_id: stream,
                detected_at: 1,
                old_version_id: old,
                start_time: 8_000.0,
                delta: 30.0,
                dvr_deadline_at: 9,
            })
            .unwrap();
        (database, change, old, new)
    }

    #[test]
    fn a_provisional_change_has_only_the_old_bar() {
        let (database, change, old, _) = database_with_change();

        let found = comparison(&database, change).unwrap().unwrap();

        assert_eq!(found.state, "provisional");
        assert_eq!(
            found.old.map(|bar| (bar.version_id, bar.start)),
            Some((old, 6_340.0))
        );
        assert_eq!(found.new, None);
        assert_eq!(found.checks, json!([]));
    }

    #[test]
    fn a_confirmed_change_has_both_bars_and_its_checks() {
        let (database, change, old, new) = database_with_change();
        database
            .confirm_change(&Confirmation {
                change_id: change,
                new_version_id: new,
                checks: "[{\"name\":\"a\"}]".to_string(),
                boundaries: "[9100.0]".to_string(),
                regions: vec![
                    NewRegion {
                        class: "new".to_string(),
                        start: 9_000.0,
                        end: 9_200.0,
                        match_version: None,
                        match_start: None,
                    },
                    NewRegion {
                        class: "deleted".to_string(),
                        start: 6_500.0,
                        end: 6_600.0,
                        match_version: None,
                        match_start: None,
                    },
                ],
            })
            .unwrap();

        let found = comparison(&database, change).unwrap().unwrap();

        let old_bar = found.old.unwrap();
        let new_bar = found.new.unwrap();
        assert_eq!(old_bar.version_id, old);
        assert_eq!(old_bar.regions.len(), 1);
        assert_eq!(new_bar.start, 9_000.0);
        assert_eq!(new_bar.regions[0].video_boundaries, [9_100.0]);
        assert_eq!(found.checks, json!([{ "name": "a" }]));
    }

    #[test]
    fn an_unknown_change_has_no_comparison() {
        let (database, ..) = database_with_change();

        assert_eq!(comparison(&database, 99).unwrap(), None);
    }
}

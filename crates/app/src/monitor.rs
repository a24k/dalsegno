//! One pass of analysis over a stream's stored envelope (D-08, D-25).
//!
//! The database is locked to read and to write, and not during the CPU work between, so a pass
//! never holds up following.

use crate::error::AcquireError;
use crate::fetch::{SharedDatabase, lock};
use crate::lifecycle::PrefetchTarget;
use dalsegno_core::analysis::{
    Diff, DiffConfig, LagRun, PeriodConfig, PeriodEstimate, Prepared, Region, RunKind, Series,
    Signal, TrackConfig, VersionCycle, compare, detect_signals, estimate_period, lag_bounds,
    summarise, track,
};
use dalsegno_core::features::BIN_RATE;
use dalsegno_store::changes::{
    AnalysisState, Confirmation, NewChange, NewRegion, NewVersion, OpenChange, StoredRun,
    VersionRow,
};
use dalsegno_store::database::Database;
use dalsegno_store::retention::{HoldOwner, NewHold};
use dalsegno_store::streams::StreamRow;
use serde::Serialize;
use serde_json::json;

/// The DVR window, which is how much history a pass can need.
const HISTORY_SECONDS: f64 = 120.0 * 3600.0;
/// Each pass goes over the end of the last one, as a signal needs the points before it.
const OVERLAP_SECONDS: f64 = 3600.0;
/// A cycle counts as seen this long after it could have ended.
const CYCLE_MARGIN_SECONDS: f64 = 600.0;
/// The old cycle ends this long before a change, as comparison searches 10 minutes past a
/// cycle's ends and must not find the new audio there. It loops, so any whole cycle will do.
const OLD_CYCLE_CLEARANCE_SECONDS: f64 = 660.0;
/// Net changes this close are one change.
const DELTA_TOLERANCE: f64 = 5.0;
/// Video is fetched this far either side of a change.
const PREFETCH_MARGIN_SECONDS: f64 = 300.0;
const DVR_MS: i64 = 120 * 3600 * 1000;
const MILLIS_PER_SECOND: f64 = 1_000.0;

/// What a pass did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct MonitorReport {
    /// The period, if there is one.
    pub period: Option<f64>,
    /// The change this pass opened.
    pub opened: Option<i64>,
    /// The change this pass confirmed.
    pub confirmed: Option<i64>,
}

fn read_history(database: &Database, stream_id: i64) -> Result<Option<Series>, AcquireError> {
    let Some(last) = database.envelope_last_bin(stream_id)? else {
        return Ok(None);
    };
    let first = last.saturating_sub((HISTORY_SECONDS * BIN_RATE) as u64);
    let rows = database.envelope_rows_since(stream_id, first)?;
    let Some(start) = rows.first().map(|row| row.first_bin) else {
        return Ok(None);
    };
    let end = rows
        .iter()
        .map(|row| row.first_bin + row.values.len() as u64)
        .max()
        .unwrap_or(start);
    let length = (end - start) as usize;
    let mut series = Series {
        start_time: start as f64 / BIN_RATE,
        rate: BIN_RATE,
        values: vec![0.0; length],
        valid: vec![false; length],
    };
    for row in rows {
        let offset = (row.first_bin - start) as usize;
        let span = offset..offset + row.values.len();
        series.values[span.clone()].copy_from_slice(&row.values);
        series.valid[span].fill(true);
    }
    Ok(Some(series))
}

/// The change signals of a pass, reduced to what the lifecycle needs.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Seen {
    first: f64,
    last: f64,
    delta: Option<f64>,
    range: (f64, f64),
}

fn summarise_signals(signals: &[Signal]) -> Option<Seen> {
    signals.iter().fold(None, |seen, signal| {
        let (start, end, delta) = match *signal {
            Signal::LagShift { start, delta, .. } => (start, start, Some(delta)),
            Signal::Unmatched { start, end } => (start, end, None),
            Signal::LostTrack { .. } => return seen,
        };
        Some(match seen {
            None => Seen {
                first: start,
                last: start,
                delta,
                range: (start, end),
            },
            Some(earlier) => Seen {
                first: earlier.first.min(start),
                last: earlier.last.max(start),
                delta: delta.or(earlier.delta),
                range: (earlier.range.0.min(start), earlier.range.1.max(end)),
            },
        })
    })
}

fn kind_text(kind: RunKind) -> &'static str {
    match kind {
        RunKind::Lag => "lag",
        RunKind::Outlier => "outlier",
        RunKind::Unmatched => "unmatched",
        RunKind::NoData => "no_data",
    }
}

fn stored_runs(runs: &[LagRun]) -> Vec<StoredRun> {
    runs.iter()
        .map(|run| StoredRun {
            start: run.start,
            end: run.end,
            kind: kind_text(run.kind),
            lag: run.lag,
        })
        .collect()
}

fn class_text(region: &Region) -> Result<String, AcquireError> {
    let value = serde_json::to_value(region.class).map_err(|_| AcquireError::Task)?;
    value.as_str().map(str::to_string).ok_or(AcquireError::Task)
}

fn to_json<T: Serialize>(value: &T) -> Result<String, AcquireError> {
    serde_json::to_string(value).map_err(|_| AcquireError::Task)
}

/// What a pass works on, once the stream's state is read.
struct Pass<'a> {
    database: &'a SharedDatabase,
    row: &'a StreamRow,
    data: &'a Prepared,
    now: i64,
}

/// Runs one pass for a stream.
pub fn run_pass(
    database: &SharedDatabase,
    row: &StreamRow,
    now: i64,
) -> Result<MonitorReport, AcquireError> {
    let (series, period, state, program) = {
        let database = lock(database)?;
        (
            read_history(&database, row.id)?,
            database.stream_period(row.id)?,
            database.analysis_state(row.id)?,
            database
                .program_of_stream(row.id)?
                .map(|program| program.id),
        )
    };
    let Some(series) = series else {
        return Ok(MonitorReport::default());
    };
    let data = Prepared::new(&series);
    let pass = Pass {
        database,
        row,
        data: &data,
        now,
    };
    let Some(period) = period else {
        return pass.find_first_period(program);
    };
    let seen = pass.track(period, state)?;
    let report = MonitorReport {
        period: Some(period),
        ..MonitorReport::default()
    };
    match program {
        Some(program) => pass.follow_change(program, period, seen, report),
        None => Ok(report),
    }
}

impl Pass<'_> {
    /// Tracks what is new, stores it, and returns the signals.
    fn track(
        &self,
        period: f64,
        state: Option<AnalysisState>,
    ) -> Result<Option<Seen>, AcquireError> {
        let config = TrackConfig::default();
        let end = self.data.end_time() - config.window;
        let from = state
            .map_or(end - period, |state| state.tracked_until - OVERLAP_SECONDS)
            .max(self.data.time_of(0));
        let widened = state.is_some_and(|state| state.widened);
        let points = track(self.data, period, from..end, &config, widened);
        let runs = summarise(&points, &config);
        let signals = detect_signals(self.data, &runs, period, &config);
        let tracking = AnalysisState {
            tracked_until: end,
            widened: signals
                .iter()
                .any(|signal| matches!(signal, Signal::LostTrack { .. })),
        };
        let stored = stored_runs(&runs);
        lock(self.database)?
            .atomically(|database| database.save_tracking(self.row.id, from, &stored, tracking))?;
        Ok(summarise_signals(&signals))
    }

    fn follow_change(
        &self,
        program: i64,
        period: f64,
        seen: Option<Seen>,
        report: MonitorReport,
    ) -> Result<MonitorReport, AcquireError> {
        let database = lock(self.database)?;
        let version =
            current_or_first_version(&database, self.row, program, self.data, period, self.now)?;
        let Some(version) = version else {
            return Ok(report);
        };
        let Some(change) = database.open_change(program)? else {
            let opened = seen
                .map(|seen| self.open(&database, program, version, seen))
                .transpose()?;
            return Ok(MonitorReport { opened, ..report });
        };
        let change = with_later_signal(&database, change, seen)?;
        drop(database);
        let confirmed = self.try_confirm(program, period, change, version)?;
        Ok(MonitorReport {
            confirmed,
            ..report
        })
    }

    fn find_first_period(&self, program: Option<i64>) -> Result<MonitorReport, AcquireError> {
        let estimate = estimate_period(self.data, &PeriodConfig::default());
        let database = lock(self.database)?;
        match estimate {
            PeriodEstimate::Found { period, .. } => {
                // Tracking starts here, or the first change would be dated by the next pass.
                let tracking = AnalysisState {
                    tracked_until: self.data.end_time() - TrackConfig::default().window,
                    widened: false,
                };
                database.atomically(|database| {
                    database.set_stream_period(self.row.id, period)?;
                    database.save_tracking(self.row.id, tracking.tracked_until, &[], tracking)?;
                    if let Some(program) = program {
                        current_or_first_version(
                            database, self.row, program, self.data, period, self.now,
                        )?;
                    }
                    Ok::<_, AcquireError>(())
                })?;
                Ok(MonitorReport {
                    period: Some(period),
                    ..MonitorReport::default()
                })
            }
            PeriodEstimate::NoPeriod { testable_up_to } => {
                database.set_testable_period(self.row.id, testable_up_to)?;
                Ok(MonitorReport::default())
            }
        }
    }

    /// Opens a change, with its notification and its prefetch, or none of them.
    fn open(
        &self,
        database: &Database,
        program: i64,
        version: VersionRow,
        seen: Seen,
    ) -> Result<i64, AcquireError> {
        // The DVR expires oldest-first, so the old version's last cycle binds (D-23).
        let last_cycle_start = seen.first - version.period;
        let age_ms = ((self.data.end_time() - last_cycle_start) * MILLIS_PER_SECOND) as i64;
        let change = NewChange {
            program_id: program,
            stream_id: self.row.id,
            detected_at: self.now,
            old_version_id: version.id,
            start_time: seen.first,
            delta: seen.delta.unwrap_or(0.0),
            dvr_deadline_at: self.now + DVR_MS - age_ms,
        };
        let body = to_json(&json!({
            "stream": self.row.stream.video_id,
            "started_at": seen.first,
            "delta": change.delta,
            "dvr_deadline_at": change.dvr_deadline_at,
        }))?;
        database.atomically(|database| {
            let id = database.insert_change(&change)?;
            let subject = format!("change:{id}:provisional");
            database.enqueue_notification("change", &subject, &body, self.now)?;
            queue_prefetch(database, self.row, id, seen, self.now)?;
            Ok::<_, AcquireError>(id)
        })
    }

    /// Confirms a change once a whole cycle has passed without another signal. Its version,
    /// comparison and notification are written together.
    fn try_confirm(
        &self,
        program: i64,
        period: f64,
        change: OpenChange,
        version: VersionRow,
    ) -> Result<Option<i64>, AcquireError> {
        let old = lock(self.database)?
            .version(change.old_version_id)?
            .unwrap_or(version);
        let Some(closing) = compare_cycles(self.data, period, &change, &old) else {
            return Ok(None);
        };
        let regions = stored_regions(&closing.diff)?;
        let checks = to_json(&closing.diff.checks)?;
        let boundaries = to_json(&closing.diff.video_boundaries)?;
        let body = to_json(&json!({
            "stream": self.row.stream.video_id,
            "old_period": old.period,
            "new_period": closing.period,
            "regions": regions.len(),
        }))?;
        let (row, now) = (self.row, self.now);
        lock(self.database)?.atomically(|database| {
            let new_version = database.insert_version(&NewVersion {
                program_id: program,
                stream_id: row.id,
                period: closing.period,
                anchor_time: closing.start,
                seen_at: now,
                on_air_from: change.start_time,
            })?;
            let last_cycle_start = change.start_time - old.period;
            database.supersede_version(old.id, change.start_time, last_cycle_start)?;
            database.set_stream_period(row.id, closing.period)?;
            database.confirm_change(&Confirmation {
                change_id: change.id,
                new_version_id: new_version,
                checks,
                boundaries,
                regions,
            })?;
            let subject = format!("change:{}:confirmed", change.id);
            database.enqueue_notification("change", &subject, &body, now)?;
            hold_cycle(database, row, new_version, closing.start, closing.period)
        })?;
        Ok(Some(change.id))
    }
}

/// Returns the version on air, recording the first one when a program has none yet.
fn current_or_first_version(
    database: &Database,
    row: &StreamRow,
    program: i64,
    data: &Prepared,
    period: f64,
    now: i64,
) -> Result<Option<VersionRow>, AcquireError> {
    if let Some(version) = database.current_version(program)? {
        return Ok(Some(version));
    }
    let anchor = data.end_time() - period;
    if anchor < data.time_of(0) {
        return Ok(None);
    }
    database.insert_version(&NewVersion {
        program_id: program,
        stream_id: row.id,
        period,
        anchor_time: anchor,
        seen_at: now,
        on_air_from: anchor,
    })?;
    Ok(database.current_version(program)?)
}

/// Queues the video of a change, when its audio is stored to place it.
fn queue_prefetch(
    database: &Database,
    row: &StreamRow,
    change: i64,
    seen: Seen,
    now: i64,
) -> Result<(), AcquireError> {
    let audio = row.stream.audio_itag;
    let first = database.fragment_at_time(row.id, audio, seen.range.0 - PREFETCH_MARGIN_SECONDS)?;
    let last = database.fragment_at_time(row.id, audio, seen.range.1 + PREFETCH_MARGIN_SECONDS)?;
    if let (Some(first), Some(last), Some(_)) = (first, last, row.stream.video_itag) {
        let target = PrefetchTarget {
            stream: row.stream.video_id.clone(),
            change,
            first,
            last,
        };
        database.enqueue_job(&target.job()?, now)?;
    }
    Ok(())
}

fn with_later_signal(
    database: &Database,
    change: OpenChange,
    seen: Option<Seen>,
) -> Result<OpenChange, AcquireError> {
    let Some(seen) = seen else {
        return Ok(change);
    };
    // A settled lag keeps signalling from the start of each pass, so only a new delta counts.
    let has_new_delta = seen
        .delta
        .is_some_and(|delta| (delta - change.last_delta).abs() > DELTA_TOLERANCE);
    let is_later = seen.delta.is_none() && seen.last > change.last_signal_time;
    if !has_new_delta && !is_later {
        return Ok(change);
    }
    let delta = seen.delta.unwrap_or(change.last_delta);
    database.update_change_signal(change.id, seen.last, delta)?;
    Ok(OpenChange {
        last_signal_time: seen.last,
        last_delta: delta,
        ..change
    })
}

/// The comparison that closes a change.
struct Closing {
    period: f64,
    start: f64,
    diff: Diff,
}

fn compare_cycles(
    data: &Prepared,
    period: f64,
    change: &OpenChange,
    old: &VersionRow,
) -> Option<Closing> {
    let bounds = lag_bounds(period);
    let search = PeriodConfig {
        min_period: bounds.start.max(PeriodConfig::default().min_period),
        max_period: bounds.end,
        ..PeriodConfig::default()
    };
    let PeriodEstimate::Found { period: new, .. } = estimate_period(data, &search) else {
        return None;
    };
    let is_cycle_seen = data.end_time() >= change.last_signal_time + new + CYCLE_MARGIN_SECONDS;
    let old_start = change.start_time - old.period - OLD_CYCLE_CLEARANCE_SECONDS;
    if !is_cycle_seen || old_start < data.time_of(0) {
        return None;
    }
    let config = DiffConfig::default();
    let start = data.end_time() - new - config.window;
    let versions = [VersionCycle {
        id: old.id,
        data,
        start: old_start,
        period: old.period,
    }];
    Some(Closing {
        period: new,
        start,
        diff: compare(data, start, new, &versions, &[], &config),
    })
}

/// Holds the audio of a version's cycle for good, when it is stored.
fn hold_cycle(
    database: &Database,
    row: &StreamRow,
    version: i64,
    start: f64,
    period: f64,
) -> Result<(), AcquireError> {
    let audio = row.stream.audio_itag;
    let first = database.fragment_at_time(row.id, audio, start)?;
    let last = database.fragment_at_time(row.id, audio, start + period)?;
    if let (Some(sq_first), Some(sq_last)) = (first, last) {
        database.insert_hold(&NewHold {
            stream_id: row.id,
            itag: audio,
            sq_first,
            sq_last,
            owner: HoldOwner::Version(version),
        })?;
    }
    Ok(())
}

fn stored_regions(diff: &Diff) -> Result<Vec<NewRegion>, AcquireError> {
    diff.regions
        .iter()
        .map(|region| {
            Ok(NewRegion {
                class: class_text(region)?,
                start: region.start,
                end: region.end,
                match_version: region.version,
                match_start: region.match_start,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dalsegno_store::features::EnvelopeRow;
    use dalsegno_store::programs::NewProgram;
    use dalsegno_store::streams::NewStream;
    use std::sync::{Arc, Mutex};

    const PERIOD: usize = 30_000;
    const ROW_BINS: usize = 600;

    /// Smoothed noise, which matches only itself.
    fn noise(seed: u64, bins: usize) -> Vec<f32> {
        let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut level = 0.0_f32;
        (0..bins)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                let value = (state >> 40) as f32 / (1u64 << 24) as f32 - 0.5;
                level = 0.8 * level + 0.2 * value * 8.0;
                8.0 + level
            })
            .collect()
    }

    struct Fixture {
        database: SharedDatabase,
        row: StreamRow,
        program: i64,
        next_sq: u64,
        next_bin: u64,
    }

    impl Fixture {
        fn new() -> Self {
            let database = Database::open_in_memory().unwrap();
            let stream = NewStream {
                video_id: "abcDEF123_-".to_string(),
                title: "Example TV".to_string(),
                channel_id: None,
                channel_name: None,
                thumbnail_url: None,
                video_itag: Some(248),
                audio_itag: 140,
                fragment_len: 5.0,
                registered_at: 0,
            };
            let id = database.insert_stream(&stream).unwrap();
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
            database.link_stream_to_program(id, program).unwrap();
            Self {
                database: Arc::new(Mutex::new(database)),
                row: StreamRow { id, stream },
                program,
                next_sq: 1,
                next_bin: 0,
            }
        }

        fn store(&mut self, values: &[f32]) {
            let mut database = lock(&self.database).unwrap();
            let run = database
                .insert_feature_run(self.row.id, self.next_sq, 0.0)
                .unwrap();
            for chunk in values.chunks(ROW_BINS) {
                let row = EnvelopeRow {
                    sq: self.next_sq,
                    first_bin: self.next_bin,
                    values: chunk.to_vec(),
                };
                database.advance_feature_run(&run, &row).unwrap();
                self.next_sq += 1;
                self.next_bin += chunk.len() as u64;
            }
        }

        fn pass(&self) -> MonitorReport {
            run_pass(&self.database, &self.row, 1_000).unwrap()
        }

        fn count(&self, sql: &str) -> i64 {
            let database = lock(&self.database).unwrap();
            database
                .connection()
                .query_row(sql, [], |row| row.get(0))
                .unwrap()
        }
    }

    fn inserted(cycle: &[f32], seed: u64) -> Vec<f32> {
        [
            &cycle[..PERIOD / 2],
            &noise(seed, 5_000),
            &cycle[PERIOD / 2..],
        ]
        .concat()
    }

    #[test]
    fn a_stream_with_no_envelope_does_nothing() {
        assert_eq!(Fixture::new().pass(), MonitorReport::default());
    }

    #[test]
    fn the_first_period_records_the_first_version() {
        let mut fixture = Fixture::new();
        let cycle = noise(1, PERIOD);
        fixture.store(&[&cycle[..], &cycle, &cycle].concat());

        let report = fixture.pass();

        assert!((report.period.unwrap() - 3_000.0).abs() < 1.0, "{report:?}");
        assert_eq!(fixture.count("SELECT COUNT(*) FROM versions"), 1);
    }

    #[test]
    fn audio_with_no_loop_records_how_far_it_could_see() {
        let mut fixture = Fixture::new();
        fixture.store(&noise(2, 30_000));

        assert_eq!(fixture.pass().period, None);
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM streams WHERE testable_period_max IS NOT NULL"),
            1
        );
    }

    #[test]
    fn an_unchanged_cycle_opens_nothing() {
        let mut fixture = Fixture::new();
        let cycle = noise(3, PERIOD);
        fixture.store(&[&cycle[..], &cycle, &cycle].concat());
        fixture.pass();
        fixture.store(&cycle);

        assert_eq!(fixture.pass().opened, None);
        assert_eq!(fixture.count("SELECT COUNT(*) FROM changes"), 0);
    }

    #[test]
    fn a_change_opens_with_its_notification_and_closes_a_cycle_later() {
        let mut fixture = Fixture::new();
        let cycle = noise(4, PERIOD);
        fixture.store(&[&cycle[..], &cycle, &cycle].concat());
        fixture.pass();
        let changed = inserted(&cycle, 5);
        fixture.store(&changed);

        let opened = fixture.pass().opened.expect("a change");

        assert_eq!(fixture.count("SELECT COUNT(*) FROM notifications"), 1);
        fixture.store(&[&changed[..], &changed].concat());
        let report = fixture.pass();
        assert_eq!(report.confirmed, Some(opened), "{report:?}");
        assert_eq!(fixture.count("SELECT COUNT(*) FROM notifications"), 2);
        assert_eq!(fixture.count("SELECT COUNT(*) FROM versions"), 2);
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM change_regions WHERE class = 'new'"),
            1
        );
        let new_start: f64 = lock(&fixture.database)
            .unwrap()
            .connection()
            .query_row(
                "SELECT start_time FROM change_regions WHERE class = 'new'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        // In the latest cycle, two cycles after the insertion began 10,500 s in.
        assert!((new_start - 17_500.0).abs() < 60.0, "{new_start}");
        let database = lock(&fixture.database).unwrap();
        let current = database.current_version(fixture.program).unwrap().unwrap();
        assert!((current.period - 3_500.0).abs() < 1.0, "{current:?}");
        assert_eq!(database.open_change(fixture.program).unwrap(), None);
    }

    #[test]
    fn a_later_pass_leaves_a_confirmed_change_alone() {
        let mut fixture = Fixture::new();
        let cycle = noise(6, PERIOD);
        fixture.store(&[&cycle[..], &cycle, &cycle].concat());
        fixture.pass();
        let changed = inserted(&cycle, 7);
        fixture.store(&changed);
        fixture.pass();
        fixture.store(&[&changed[..], &changed].concat());
        assert!(fixture.pass().confirmed.is_some());
        fixture.store(&changed);

        let report = fixture.pass();

        assert_eq!((report.opened, report.confirmed), (None, None));
        assert_eq!(fixture.count("SELECT COUNT(*) FROM changes"), 1);
    }

    #[test]
    fn a_stream_with_no_program_tracks_and_opens_nothing() {
        let mut fixture = Fixture::new();
        lock(&fixture.database)
            .unwrap()
            .connection()
            .execute("UPDATE streams SET program_id = NULL", [])
            .unwrap();
        let cycle = noise(8, PERIOD);
        fixture.store(&[&cycle[..], &cycle, &cycle].concat());
        fixture.pass();
        fixture.store(&inserted(&cycle, 9));

        assert_eq!(fixture.pass().opened, None);
        assert!(fixture.count("SELECT COUNT(*) FROM lag_runs") > 0);
    }

    #[test]
    fn a_gap_in_the_envelope_is_marked_invalid() {
        let mut fixture = Fixture::new();
        fixture.store(&noise(10, 1_000));
        fixture.next_bin += 500;
        fixture.next_sq += 1;
        fixture.store(&noise(11, 1_000));

        let database = lock(&fixture.database).unwrap();
        let series = read_history(&database, fixture.row.id).unwrap().unwrap();

        assert_eq!(series.values.len(), 2_500);
        assert!(!series.valid[1_200]);
        assert!(series.valid[1_600]);
    }
}

//! Extracting the envelope, run by run (D-07).
//!
//! Each contiguous stretch of audio has its own run and cursor, so an open gap neither blocks
//! later audio nor is skipped for good. Progress is committed per fragment.

use crate::error::AcquireError;
use dalsegno_core::container::{Mp4Fragment, parse_mp4_fragment};
use dalsegno_core::features::EnvelopeExtractor;
use dalsegno_io::fragments::FragmentStore;
use dalsegno_store::database::Database;
use dalsegno_store::features::{EnvelopeRow, FeatureRun};
use dalsegno_store::streams::StreamRow;
use serde::Serialize;
use std::collections::{BTreeSet, HashSet};

const MAX_DRIFT_SECONDS: f64 = 0.05;

/// What one extraction pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FeatureReport {
    /// Fragments processed.
    pub fragments: u64,
    /// Bins stored.
    pub bins: u64,
    /// Runs after this pass.
    pub runs: u64,
    /// Fragments that could not be read or decoded.
    pub unreadable: Vec<u64>,
}

enum Priming {
    /// The run has processed nothing, so it starts at its first fragment.
    Unstarted,
    /// The decoder is ready for the run's next fragment.
    Ready(Box<EnvelopeExtractor>),
    /// The fragment before the next one is gone, so the run cannot continue.
    Impossible,
}

struct Pass<'a> {
    database: &'a mut Database,
    store: &'a FragmentStore,
    stream: &'a StreamRow,
    available: BTreeSet<u64>,
    stalled: HashSet<i64>,
    report: FeatureReport,
}

/// Brings every run up to date with the stored audio.
pub fn extract_features(
    database: &mut Database,
    store: &FragmentStore,
    stream: &StreamRow,
) -> Result<FeatureReport, AcquireError> {
    let itag = stream.stream.audio_itag;
    let Some((low, high)) = database.fragment_bounds(stream.id, itag)? else {
        return Ok(FeatureReport::default());
    };
    let available = database
        .fragments_in_range(stream.id, itag, low, high)?
        .iter()
        .map(|row| row.sq)
        .collect();
    let mut pass = Pass {
        database,
        store,
        stream,
        available,
        stalled: HashSet::new(),
        report: FeatureReport::default(),
    };
    while pass.step()? {}
    pass.report.runs = pass.database.feature_runs(stream.id)?.len() as u64;
    Ok(pass.report)
}

impl Pass<'_> {
    /// Advances one run, and returns whether anything was left to try.
    fn step(&mut self) -> Result<bool, AcquireError> {
        let runs = self.database.feature_runs(self.stream.id)?;
        let waiting = runs.iter().enumerate().find(|(_, run)| {
            self.available.contains(&run.next_sq) && !self.stalled.contains(&run.id)
        });
        if let Some((index, run)) = waiting {
            self.advance(run, runs.get(index + 1))?;
            return Ok(true);
        }
        let Some(sq) = self.first_uncovered(&runs) else {
            return Ok(false);
        };
        self.start_run(sq)?;
        Ok(true)
    }

    fn first_uncovered(&self, runs: &[FeatureRun]) -> Option<u64> {
        self.available.iter().copied().find(|sq| {
            !runs
                .iter()
                .any(|run| (run.sq_first..run.next_sq.max(run.sq_first + 1)).contains(sq))
        })
    }

    fn start_run(&mut self, sq: u64) -> Result<(), AcquireError> {
        let Some(bytes) = self.read(sq) else {
            return Ok(());
        };
        match parse_mp4_fragment(&bytes) {
            Ok(fragment) => {
                self.database
                    .insert_feature_run(self.stream.id, sq, fragment.start_time())?;
            }
            Err(error) => self.give_up_on(sq, &error.to_string()),
        }
        Ok(())
    }

    fn read(&mut self, sq: u64) -> Option<Vec<u8>> {
        match self
            .store
            .read(self.stream.id, self.stream.stream.audio_itag, sq)
        {
            Ok(bytes) => Some(bytes),
            Err(error) => {
                self.give_up_on(sq, &error.to_string());
                None
            }
        }
    }

    fn give_up_on(&mut self, sq: u64, reason: &str) {
        tracing::warn!(stream = self.stream.id, sq, reason, "fragment skipped");
        self.available.remove(&sq);
        self.report.unreadable.push(sq);
    }

    fn advance(
        &mut self,
        run: &FeatureRun,
        following: Option<&FeatureRun>,
    ) -> Result<(), AcquireError> {
        let mut extractor = match self.prime(run)? {
            Priming::Unstarted => None,
            Priming::Ready(extractor) => Some(*extractor),
            Priming::Impossible => return self.end_run(run, following),
        };
        let mut sq = run.next_sq;
        while self.available.contains(&sq) {
            let Some(bytes) = self.read(sq) else {
                return Ok(());
            };
            let fragment = match parse_mp4_fragment(&bytes) {
                Ok(fragment) => fragment,
                Err(error) => {
                    self.give_up_on(sq, &error.to_string());
                    return Ok(());
                }
            };
            let extractor = match &mut extractor {
                Some(extractor) if has_drifted(extractor, &fragment) => {
                    tracing::warn!(stream = self.stream.id, sq, "stream time jumped");
                    let continued = FeatureRun {
                        next_sq: sq,
                        ..run.clone()
                    };
                    return self.end_run(&continued, following);
                }
                Some(extractor) => extractor,
                slot @ None => match EnvelopeExtractor::new(&fragment) {
                    Ok(started) => slot.insert(started),
                    Err(error) => {
                        self.give_up_on(sq, &error.to_string());
                        return Ok(());
                    }
                },
            };
            let bins = match extractor.process(&fragment) {
                Ok(bins) => bins,
                Err(error) => {
                    self.give_up_on(sq, &error.to_string());
                    return Ok(());
                }
            };
            self.report.fragments += 1;
            self.report.bins += bins.values.len() as u64;
            let row = EnvelopeRow {
                sq,
                first_bin: bins.first_bin,
                values: bins.values,
            };
            self.database.advance_feature_run(run, &row)?;
            if let Some(next) = following.filter(|next| next.sq_first == sq) {
                self.database.merge_feature_runs(run, next)?;
                return Ok(());
            }
            sq += 1;
        }
        Ok(())
    }

    /// Re-primes the decoder on the fragment before the run's next one.
    fn prime(&mut self, run: &FeatureRun) -> Result<Priming, AcquireError> {
        if run.next_sq == run.sq_first {
            return Ok(Priming::Unstarted);
        }
        let previous = run.next_sq - 1;
        let Some(end_bin) = self.database.envelope_end_bin(self.stream.id, previous)? else {
            return Ok(Priming::Impossible);
        };
        let itag = self.stream.stream.audio_itag;
        let Ok(bytes) = self.store.read(self.stream.id, itag, previous) else {
            return Ok(Priming::Impossible);
        };
        let Ok(fragment) = parse_mp4_fragment(&bytes) else {
            return Ok(Priming::Impossible);
        };
        let Ok(mut extractor) = EnvelopeExtractor::new(&fragment) else {
            return Ok(Priming::Impossible);
        };
        extractor.skip_before(end_bin);
        match extractor.process(&fragment) {
            Ok(_) => Ok(Priming::Ready(Box::new(extractor))),
            Err(_) => Ok(Priming::Impossible),
        }
    }

    /// Closes a run that cannot continue, with a new run taking over at `run.next_sq`.
    fn end_run(
        &mut self,
        run: &FeatureRun,
        following: Option<&FeatureRun>,
    ) -> Result<(), AcquireError> {
        self.stalled.insert(run.id);
        if following.is_none_or(|next| next.sq_first != run.next_sq) {
            self.start_run(run.next_sq)?;
        }
        Ok(())
    }
}

fn has_drifted(extractor: &EnvelopeExtractor, fragment: &Mp4Fragment<'_>) -> bool {
    let limit = MAX_DRIFT_SECONDS * f64::from(fragment.timescale);
    extractor.end_tick().abs_diff(fragment.start_ticks) as f64 > limit
}

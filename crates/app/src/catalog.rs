//! Programs and known material, which a person manages through the API (FR-S3, FR-K1).

use crate::acquisition::Library;
use crate::error::AcquireError;
use crate::fetch::lock;
use dalsegno_core::export::{NameError, check_output_dir, check_template};
use dalsegno_core::features::BIN_RATE;
use dalsegno_store::features::{ENVELOPE_FEATURE, EnvelopeRow};
use dalsegno_store::known::{KnownClipRow, NewKnownClip};
use dalsegno_store::programs::NewProgram;

/// Why a program cannot be stored.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProgramError {
    /// The short name starts every output name.
    #[error("the short name is empty")]
    EmptyShortName,
    /// A name, folder or template is unusable.
    #[error("{0}")]
    Name(#[from] NameError),
}

/// Checks that a program can name its output, before anything is stored.
pub fn check_program(program: &NewProgram) -> Result<(), ProgramError> {
    if program.short_name.trim().is_empty() {
        return Err(ProgramError::EmptyShortName);
    }
    check_output_dir(&program.output_dir)?;
    check_template(
        &program.name_template,
        &program.short_name,
        &program.addition_suffix,
    )?;
    Ok(())
}

/// A stretch of a stream to keep as known material.
#[derive(Debug, Clone, PartialEq)]
pub struct KnownStretch<'a> {
    /// What the operator calls it.
    pub name: &'a str,
    /// The stream it is cut from.
    pub stream_id: i64,
    /// Stream time it starts, in seconds.
    pub start: f64,
    /// Stream time it ends, in seconds.
    pub end: f64,
}

/// Returns the values of `first..end` on the grid, if the rows cover every bin.
fn cut_bins(rows: &[EnvelopeRow], first: u64, end: u64) -> Option<Vec<f32>> {
    let mut values = Vec::with_capacity((end - first) as usize);
    let mut next = first;
    for row in rows {
        if row.first_bin > next {
            return None;
        }
        for (bin, value) in (row.first_bin..).zip(&row.values) {
            if bin == next && next < end {
                values.push(*value);
                next += 1;
            }
        }
    }
    (next == end).then_some(values)
}

impl Library {
    /// Makes a stretch of a stream's stored envelope known material of a program. The clip is
    /// stored for analysis to leave out later.
    pub fn add_known_clip(
        &self,
        program_id: i64,
        stretch: &KnownStretch<'_>,
    ) -> Result<KnownClipRow, AcquireError> {
        let first = (stretch.start * BIN_RATE).ceil() as u64;
        let end = (stretch.end * BIN_RATE).floor() as u64;
        if end <= first {
            return Err(AcquireError::InvalidRequest(
                "the stretch is shorter than one envelope value",
            ));
        }
        let database = lock(&self.database)?;
        let rows = database.envelope_rows_between(stretch.stream_id, first, end)?;
        let values = cut_bins(&rows, first, end).ok_or(AcquireError::InvalidRequest(
            "the stored envelope does not cover that stretch",
        ))?;
        Ok(database.insert_known_clip(
            program_id,
            &NewKnownClip {
                name: stretch.name,
                duration: stretch.end - stretch.start,
                feature: ENVELOPE_FEATURE,
                values: &values,
            },
        )?)
    }

    /// Makes a stream part of a program. A stream keeps one program for the versions it has
    /// found, and analysis follows one stream for each program, so a link that would break
    /// either is refused.
    pub fn link_stream(&self, stream_id: i64, program_id: i64) -> Result<(), AcquireError> {
        let database = lock(&self.database)?;
        let current = database.program_of_stream(stream_id)?.map(|row| row.id);
        if current == Some(program_id) {
            return Ok(());
        }
        let others = database
            .version_streams(program_id)?
            .into_iter()
            .any(|stream| stream != stream_id);
        if others {
            return Err(AcquireError::InvalidRequest(
                "that program has versions from another stream, which analysis cannot continue yet",
            ));
        }
        if let Some(current) = current
            && database.version_streams(current)?.contains(&stream_id)
        {
            return Err(AcquireError::InvalidRequest(
                "the stream already has versions under its program",
            ));
        }
        Ok(database.link_stream_to_program(stream_id, program_id)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dalsegno_core::export::{DEFAULT_SUFFIX, DEFAULT_TEMPLATE};
    use dalsegno_store::changes::NewVersion;
    use dalsegno_store::streams::NewStream;
    use tempfile::{TempDir, tempdir};

    fn program(short_name: &str) -> NewProgram {
        NewProgram {
            short_name: short_name.to_string(),
            channel_id: None,
            name_template: DEFAULT_TEMPLATE.to_string(),
            addition_suffix: DEFAULT_SUFFIX.to_string(),
            artist: None,
            content_date: None,
            description: None,
            output_dir: "Example TV".to_string(),
        }
    }

    fn library() -> (TempDir, Library) {
        let root = tempdir().unwrap();
        let library = Library::open(root.path()).unwrap();
        (root, library)
    }

    fn stream(library: &Library, video_id: &str) -> i64 {
        lock(&library.database)
            .unwrap()
            .insert_stream(&NewStream {
                video_id: video_id.to_string(),
                title: "Example TV".to_string(),
                channel_id: None,
                channel_name: None,
                thumbnail_url: None,
                video_itag: None,
                audio_itag: 140,
                fragment_len: 5.0,
                registered_at: 0,
            })
            .unwrap()
    }

    fn version(library: &Library, program_id: i64, stream_id: i64) {
        lock(&library.database)
            .unwrap()
            .insert_version(&NewVersion {
                program_id,
                stream_id,
                period: 100.0,
                anchor_time: 0.0,
                seen_at: 0,
                on_air_from: 0.0,
            })
            .unwrap();
    }

    fn envelope(library: &Library, stream_id: i64, rows: &[(u64, u64, &[f32])]) {
        let mut database = lock(&library.database).unwrap();
        let run = database
            .insert_feature_run(stream_id, rows[0].0, 0.0)
            .unwrap();
        for (sq, first_bin, values) in rows {
            let row = EnvelopeRow {
                sq: *sq,
                first_bin: *first_bin,
                values: values.to_vec(),
            };
            database.advance_feature_run(&run, &row).unwrap();
        }
    }

    #[test]
    fn a_program_that_can_name_its_output_passes() {
        assert_eq!(check_program(&program("Example TV")), Ok(()));
    }

    #[test]
    fn a_program_that_cannot_name_its_output_is_refused() {
        let blank = check_program(&program("  "));
        let folder = check_program(&NewProgram {
            output_dir: "/etc".to_string(),
            ..program("Example TV")
        });
        let template = check_program(&NewProgram {
            name_template: "{name} {oops}".to_string(),
            ..program("Example TV")
        });

        assert_eq!(blank, Err(ProgramError::EmptyShortName));
        assert!(matches!(folder, Err(ProgramError::Name(_))));
        assert_eq!(
            template.unwrap_err().to_string(),
            "unknown placeholder {oops} in the name template"
        );
    }

    #[test]
    fn bins_are_cut_across_rows_and_a_gap_is_refused() {
        let rows = [
            EnvelopeRow {
                sq: 1,
                first_bin: 10,
                values: vec![1.0, 2.0],
            },
            EnvelopeRow {
                sq: 2,
                first_bin: 12,
                values: vec![3.0, 4.0],
            },
        ];
        let gap = [
            rows[0].clone(),
            EnvelopeRow {
                first_bin: 13,
                ..rows[1].clone()
            },
        ];

        assert_eq!(cut_bins(&rows, 11, 14), Some(vec![2.0, 3.0, 4.0]));
        assert_eq!(cut_bins(&rows, 11, 15), None);
        assert_eq!(cut_bins(&gap, 10, 14), None);
    }

    #[test]
    fn a_known_clip_is_cut_from_the_stored_envelope() {
        let (_root, library) = library();
        let stream_id = stream(&library, "abcDEF123_-");
        let program_id = lock(&library.database)
            .unwrap()
            .insert_program(&program("Example TV"))
            .unwrap();
        envelope(
            &library,
            stream_id,
            &[(1, 10, &[1.0, 2.0]), (2, 12, &[3.0, 4.0])],
        );

        let clip = library
            .add_known_clip(
                program_id,
                &KnownStretch {
                    name: "Opening",
                    stream_id,
                    start: 1.05,
                    end: 1.4,
                },
            )
            .unwrap();

        assert_eq!(
            (clip.name.as_str(), clip.feature.as_str()),
            ("Opening", "env10")
        );
        assert!((clip.duration - 0.35).abs() < 1e-9);
        let stored = lock(&library.database)
            .unwrap()
            .known_clips(program_id)
            .unwrap();
        assert_eq!(stored, [clip]);
    }

    #[test]
    fn a_stretch_the_envelope_does_not_cover_is_refused() {
        let (_root, library) = library();
        let stream_id = stream(&library, "abcDEF123_-");
        let program_id = lock(&library.database)
            .unwrap()
            .insert_program(&program("Example TV"))
            .unwrap();
        envelope(&library, stream_id, &[(1, 10, &[1.0, 2.0])]);

        let uncovered = library.add_known_clip(
            program_id,
            &KnownStretch {
                name: "Opening",
                stream_id,
                start: 1.0,
                end: 5.0,
            },
        );
        let tiny = library.add_known_clip(
            program_id,
            &KnownStretch {
                name: "Opening",
                stream_id,
                start: 1.01,
                end: 1.09,
            },
        );

        assert!(matches!(uncovered, Err(AcquireError::InvalidRequest(_))));
        assert!(matches!(tiny, Err(AcquireError::InvalidRequest(_))));
    }

    #[test]
    fn linking_to_the_program_a_stream_has_changes_nothing() {
        let (_root, library) = library();
        let stream_id = stream(&library, "abcDEF123_-");
        let program_id = {
            let database = lock(&library.database).unwrap();
            let id = database.insert_program(&program("Example TV")).unwrap();
            database.link_stream_to_program(stream_id, id).unwrap();
            id
        };
        version(&library, program_id, stream_id);

        library.link_stream(stream_id, program_id).unwrap();
    }

    #[test]
    fn a_stream_links_to_a_program_with_no_versions_of_another_stream() {
        let (_root, library) = library();
        let stream_id = stream(&library, "abcDEF123_-");
        let program_id = lock(&library.database)
            .unwrap()
            .insert_program(&program("Example TV"))
            .unwrap();

        library.link_stream(stream_id, program_id).unwrap();

        let database = lock(&library.database).unwrap();
        assert_eq!(
            database.program_stream_ids(program_id).unwrap(),
            [stream_id]
        );
    }

    #[test]
    fn a_program_with_versions_from_another_stream_is_not_linked() {
        let (_root, library) = library();
        let stream_id = stream(&library, "abcDEF123_-");
        let other = stream(&library, "zzzZZZ999_-");
        let program_id = lock(&library.database)
            .unwrap()
            .insert_program(&program("Example TV"))
            .unwrap();
        version(&library, program_id, other);

        assert!(matches!(
            library.link_stream(stream_id, program_id),
            Err(AcquireError::InvalidRequest(_))
        ));
    }

    #[test]
    fn a_stream_with_versions_under_its_program_is_not_moved() {
        let (_root, library) = library();
        let stream_id = stream(&library, "abcDEF123_-");
        let (own, target) = {
            let database = lock(&library.database).unwrap();
            let own = database.insert_program(&program("Own")).unwrap();
            database.link_stream_to_program(stream_id, own).unwrap();
            (own, database.insert_program(&program("Target")).unwrap())
        };
        version(&library, own, stream_id);

        let result = library.link_stream(stream_id, target);

        assert!(matches!(result, Err(AcquireError::InvalidRequest(_))));
        let database = lock(&library.database).unwrap();
        assert_eq!(
            database.program_of_stream(stream_id).unwrap().unwrap().id,
            own
        );
    }
}

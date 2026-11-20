//! Clips chosen from a change, which stay drafts until they are exported, and the operator
//! closing a change.

use crate::error::AcquireError;
use crate::export::{HOLD_AFTER_CLOSE_MS, MAX_CLIP_SECONDS};
use dalsegno_store::clips::{ClipKind, ClipState, ClipStatus, NewDraft};
use dalsegno_store::database::Database;

/// A clip to start shaping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClipRequest {
    /// The change it is chosen from.
    pub change_id: i64,
    /// What it holds.
    pub kind: ClipKind,
    /// The version it cuts, which is the change's old or new one.
    pub version_id: Option<i64>,
    /// Start of the cut, in stream time.
    pub start: f64,
    /// End of the cut, in stream time.
    pub end: f64,
}

/// What to change about a draft. What is not given stays as it is.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ClipEdit {
    /// A new kind.
    pub kind: Option<ClipKind>,
    /// A new start, in stream time.
    pub start: Option<f64>,
    /// A new end, in stream time.
    pub end: Option<f64>,
}

/// Checks a cut's times, and says what is wrong with them.
pub fn check_span(start: f64, end: f64) -> Result<(), &'static str> {
    if !start.is_finite() || !end.is_finite() {
        return Err("the times must be finite");
    }
    if start < 0.0 {
        return Err("the start must not be negative");
    }
    if start >= end {
        return Err("the start must come before the end");
    }
    if end - start > MAX_CLIP_SECONDS {
        return Err("the cut is too long");
    }
    Ok(())
}

fn checked_span(start: f64, end: f64) -> Result<(), AcquireError> {
    check_span(start, end).map_err(AcquireError::InvalidRequest)
}

fn read(database: &Database, id: i64) -> Result<ClipStatus, AcquireError> {
    database
        .clip_status(id)?
        .ok_or(AcquireError::NotFound("clip"))
}

/// Starts a draft for a change.
pub fn create_clip(database: &Database, request: &ClipRequest) -> Result<ClipStatus, AcquireError> {
    let change = database
        .change_status(request.change_id)?
        .ok_or(AcquireError::NotFound("change"))?;
    checked_span(request.start, request.end)?;
    let compared = [change.old_version_id, change.new_version_id];
    if request
        .version_id
        .is_some_and(|id| !compared.contains(&Some(id)))
    {
        return Err(AcquireError::InvalidRequest(
            "the version is not one this change compares",
        ));
    }
    let id = database.insert_draft(&NewDraft {
        kind: request.kind,
        change_id: change.id,
        version_id: request.version_id,
        stream_id: change.stream_id,
        start: request.start,
        end: request.end,
    })?;
    read(database, id)
}

/// Reshapes a draft.
pub fn edit_clip(
    database: &Database,
    id: i64,
    edit: &ClipEdit,
) -> Result<ClipStatus, AcquireError> {
    let clip = read(database, id)?;
    if clip.state != ClipState::Draft {
        return Err(AcquireError::InvalidRequest("the clip has been exported"));
    }
    let kind = edit.kind.unwrap_or(clip.kind);
    let start = edit.start.or(clip.start).unwrap_or_default();
    let end = edit.end.or(clip.end).unwrap_or_default();
    checked_span(start, end)?;
    if !database.update_draft(id, kind, start, end)? {
        return Err(AcquireError::InvalidRequest("the clip has been exported"));
    }
    read(database, id)
}

/// Closes a change on the operator's word. Its holds lapse a week later, as they do when a
/// change closes by itself.
pub fn dismiss_change(database: &Database, id: i64, now_ms: i64) -> Result<(), AcquireError> {
    database
        .change_status(id)?
        .ok_or(AcquireError::NotFound("change"))?;
    if !database.dismiss_change_by_operator(id, now_ms + HOLD_AFTER_CLOSE_MS)? {
        return Err(AcquireError::InvalidRequest("the change is already closed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dalsegno_store::changes::{NewChange, NewVersion};
    use dalsegno_store::programs::NewProgram;
    use dalsegno_store::retention::{HoldOwner, NewHold};
    use dalsegno_store::streams::NewStream;

    /// A database with one provisional change, and its old version.
    fn database() -> (Database, i64, i64) {
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
        let old = database
            .insert_version(&NewVersion {
                program_id: program,
                stream_id: stream,
                period: 1_000.0,
                anchor_time: 0.0,
                seen_at: 0,
                on_air_from: 0.0,
            })
            .unwrap();
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
        (database, change, old)
    }

    fn request(change_id: i64) -> ClipRequest {
        ClipRequest {
            change_id,
            kind: ClipKind::Cycle,
            version_id: None,
            start: 100.0,
            end: 400.0,
        }
    }

    fn message(error: AcquireError) -> String {
        error.to_string()
    }

    #[test]
    fn a_clip_starts_as_a_draft_on_the_changes_stream() {
        let (database, change, old) = database();

        let clip = create_clip(
            &database,
            &ClipRequest {
                version_id: Some(old),
                ..request(change)
            },
        )
        .unwrap();

        assert_eq!(
            (
                clip.state,
                clip.change_id,
                clip.version_id,
                clip.stream_id,
                clip.start,
                clip.end
            ),
            (
                ClipState::Draft,
                Some(change),
                Some(old),
                1,
                Some(100.0),
                Some(400.0)
            )
        );
    }

    #[test]
    fn a_clip_of_an_unknown_change_is_not_found() {
        let (database, ..) = database();

        let error = create_clip(&database, &request(99)).unwrap_err();

        assert!(matches!(error, AcquireError::NotFound("change")));
    }

    #[test]
    fn a_version_from_elsewhere_is_refused() {
        let (database, change, old) = database();

        let error = create_clip(
            &database,
            &ClipRequest {
                version_id: Some(old + 100),
                ..request(change)
            },
        )
        .unwrap_err();

        assert_eq!(
            message(error),
            "invalid request: the version is not one this change compares"
        );
    }

    #[test]
    fn spans_that_make_no_cut_are_refused() {
        let max = MAX_CLIP_SECONDS;
        let cases = [
            (5.0, 5.0),
            (6.0, 5.0),
            (-1.0, 5.0),
            (0.0, f64::NAN),
            (0.0, f64::INFINITY),
            (0.0, max + 1.0),
        ];

        for (start, end) in cases {
            assert!(check_span(start, end).is_err(), "{start} to {end}");
        }
        assert!(check_span(0.0, max).is_ok());
    }

    #[test]
    fn creating_a_clip_checks_the_span() {
        let (database, change, _) = database();

        let result = create_clip(
            &database,
            &ClipRequest {
                start: 5.0,
                end: 5.0,
                ..request(change)
            },
        );

        assert!(matches!(result, Err(AcquireError::InvalidRequest(_))));
    }

    #[test]
    fn an_edit_changes_only_what_it_names() {
        let (database, change, _) = database();
        let id = create_clip(&database, &request(change)).unwrap().id;

        let clip = edit_clip(
            &database,
            id,
            &ClipEdit {
                end: Some(500.0),
                ..ClipEdit::default()
            },
        )
        .unwrap();

        assert_eq!(
            (clip.kind, clip.start, clip.end),
            (ClipKind::Cycle, Some(100.0), Some(500.0))
        );
    }

    #[test]
    fn an_edit_is_checked_against_what_is_stored() {
        let (database, change, _) = database();
        let id = create_clip(&database, &request(change)).unwrap().id;

        let result = edit_clip(
            &database,
            id,
            &ClipEdit {
                start: Some(450.0),
                ..ClipEdit::default()
            },
        );

        assert!(matches!(result, Err(AcquireError::InvalidRequest(_))));
    }

    #[test]
    fn an_unknown_clip_cannot_be_edited() {
        let (database, ..) = database();

        let result = edit_clip(&database, 99, &ClipEdit::default());

        assert!(matches!(result, Err(AcquireError::NotFound("clip"))));
    }

    #[test]
    fn an_exported_clip_cannot_be_edited() {
        let (database, change, _) = database();
        let id = create_clip(&database, &request(change)).unwrap().id;
        database
            .connection()
            .execute(
                "UPDATE clips SET state = 'exported', name = 'Example', video_itag = 248,
                 audio_itag = 140, sq_first = 1, sq_last = 2, duration = 10.0, metadata = '{}'",
                [],
            )
            .unwrap();

        let error = edit_clip(&database, id, &ClipEdit::default()).unwrap_err();

        assert_eq!(
            message(error),
            "invalid request: the clip has been exported"
        );
    }

    #[test]
    fn a_dismissed_change_cannot_be_dismissed_again() {
        let (database, change, _) = database();
        dismiss_change(&database, change, 1_000).unwrap();

        let error = dismiss_change(&database, change, 2_000).unwrap_err();

        assert_eq!(
            message(error),
            "invalid request: the change is already closed"
        );
    }

    #[test]
    fn dismissing_lets_the_holds_lapse_a_week_later() {
        let (database, change, _) = database();
        let hold = database
            .insert_hold(&NewHold {
                stream_id: 1,
                itag: 248,
                sq_first: 1,
                sq_last: 2,
                owner: HoldOwner::Change(change),
            })
            .unwrap();

        dismiss_change(&database, change, 1_000).unwrap();

        let expires: Option<i64> = database
            .connection()
            .query_row(
                "SELECT expires_at FROM retention_holds WHERE id = ?1",
                [hold],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(expires, Some(1_000 + HOLD_AFTER_CLOSE_MS));
    }

    #[test]
    fn an_unknown_change_cannot_be_dismissed() {
        let (database, ..) = database();

        let result = dismiss_change(&database, 99, 1_000);

        assert!(matches!(result, Err(AcquireError::NotFound("change"))));
    }
}

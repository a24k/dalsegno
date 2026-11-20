//! A change in detail, its comparison, and the clips chosen from it (FR-C1).

use crate::api::{ApiError, ApiState, Envelope, change_json, ok, store_error, with_database};
use crate::operations::{invalid, parse_body};
use crate::runner::now_ms;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use dalsegno_app::comparison::comparison;
use dalsegno_app::drafts::{self, ClipEdit, ClipRequest, check_span};
use dalsegno_store::clips::{ClipKind, ClipStatus};
use dalsegno_store::database::Database;
use dalsegno_store::status::RegionStatus;
use serde::Deserialize;
use serde_json::{Value, json};

/// A clip to start shaping.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipBody {
    change: i64,
    kind: String,
    start: f64,
    end: f64,
    version: Option<i64>,
}

/// What to change about a draft.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClipPatch {
    kind: Option<String>,
    start: Option<f64>,
    end: Option<f64>,
}

fn region_json(region: &RegionStatus) -> Value {
    json!({
        "id": region.id,
        "class": region.class,
        "start": region.start,
        "end": region.end,
        "match_start": region.match_start,
    })
}

pub(crate) fn clip_json(clip: &ClipStatus) -> Value {
    json!({
        "id": clip.id,
        "state": clip.state.as_str(),
        "kind": clip.kind.as_str(),
        "change_id": clip.change_id,
        "version_id": clip.version_id,
        "stream_id": clip.stream_id,
        "start": clip.start,
        "end": clip.end,
        "name": clip.name,
        "duration": clip.duration,
        "output_path": clip.output_path,
        "plex_status": clip.plex_status.as_str(),
    })
}

fn parse_kind(text: &str) -> Result<ClipKind, ApiError> {
    ClipKind::parse(text).ok_or_else(|| invalid("the kind must be cycle or addition"))
}

/// A change with its regions and the clips chosen from it.
fn detail(database: &Database, id: i64) -> Result<Value, ApiError> {
    let change = database.change_status(id).map_err(store_error)?;
    let change = change.ok_or_else(ApiError::not_found)?;
    let regions = database.change_regions(id).map_err(store_error)?;
    let clips = database.change_clips(id).map_err(store_error)?;
    let mut body = change_json(&change);
    body["regions"] = regions.iter().map(region_json).collect();
    body["clips"] = clips.iter().map(clip_json).collect();
    Ok(body)
}

/// Returns a change, its comparison as stored, and its clips.
pub(crate) async fn change_detail(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        Ok(ok(detail(database, id)?))
    })
    .await
}

/// Returns both versions of a change, each from the start of its cycle.
pub(crate) async fn change_diff(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        let found = comparison(database, id)?.ok_or_else(ApiError::not_found)?;
        let body = serde_json::to_value(found).map_err(store_error)?;
        Ok(ok(body))
    })
    .await
}

/// Closes a change on the operator's word.
pub(crate) async fn dismiss_change(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        drafts::dismiss_change(database, id, now_ms())?;
        Ok(ok(detail(database, id)?))
    })
    .await
}

/// Starts a draft clip from a change.
pub(crate) async fn create_clip(
    State(state): State<ApiState>,
    body: Bytes,
) -> Result<(StatusCode, Json<Envelope>), ApiError> {
    let request: ClipBody = parse_body(&body)?;
    let kind = parse_kind(&request.kind)?;
    check_span(request.start, request.end).map_err(invalid)?;
    let clip = with_database(&state.database, move |database| {
        let request = ClipRequest {
            change_id: request.change,
            kind,
            version_id: request.version,
            start: request.start,
            end: request.end,
        };
        Ok(drafts::create_clip(database, &request)?)
    })
    .await?;
    Ok((StatusCode::CREATED, ok(clip_json(&clip))))
}

/// Returns a clip, draft or exported.
pub(crate) async fn clip_detail(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        let clip = database.clip_status(id).map_err(store_error)?;
        Ok(ok(clip_json(&clip.ok_or_else(ApiError::not_found)?)))
    })
    .await
}

/// Reshapes a draft clip.
pub(crate) async fn patch_clip(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<Json<Envelope>, ApiError> {
    let patch: ClipPatch = parse_body(&body)?;
    if patch.kind.is_none() && patch.start.is_none() && patch.end.is_none() {
        return Err(invalid("give at least one of kind, start and end"));
    }
    let kind = patch.kind.as_deref().map(parse_kind).transpose()?;
    if let (Some(start), Some(end)) = (patch.start, patch.end) {
        check_span(start, end).map_err(invalid)?;
    }
    let edit = ClipEdit {
        kind,
        start: patch.start,
        end: patch.end,
    };
    with_database(&state.database, move |database| {
        let clip = drafts::edit_clip(database, id, &edit)?;
        Ok(ok(clip_json(&clip)))
    })
    .await
}

#[cfg(test)]
mod tests {
    use crate::api::tests::{Fixture, fixture, get_json, send, send_body};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use dalsegno_app::fetch::lock;
    use dalsegno_store::changes::{Confirmation, NewChange, NewRegion, NewVersion};
    use dalsegno_store::fragments::FragmentRow;
    use dalsegno_store::programs::NewProgram;
    use dalsegno_store::retention::{HoldOwner, NewHold};
    use serde_json::{Value, json};

    struct Seeded {
        change: i64,
        old: i64,
        new: i64,
    }

    /// A change on the fixture's stream with an old and a new version, still provisional. The
    /// stream's audio has a fragment every 5 s from 5.0 to 500.0.
    fn seed(fixture: &Fixture) -> Seeded {
        let database = lock(&fixture.state.database).unwrap();
        for sq in 1..=100 {
            database
                .put_fragment(&FragmentRow {
                    stream_id: fixture.stream,
                    itag: 140,
                    sq,
                    stream_time: sq as f64 * 5.0,
                    duration: 5.0,
                    size: 1,
                    fetched_at: 0,
                })
                .unwrap();
        }
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
            stream_id: fixture.stream,
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
                stream_id: fixture.stream,
                detected_at: 1,
                old_version_id: old,
                start_time: 8_000.0,
                delta: 30.0,
                dvr_deadline_at: 9,
            })
            .unwrap();
        Seeded { change, old, new }
    }

    /// Confirms the change with an addition, an existing stretch and a deletion.
    fn confirm(fixture: &Fixture, seeded: &Seeded) {
        let region = |class: &str, start: f64, end: f64, matched: Option<f64>| NewRegion {
            class: class.to_string(),
            start,
            end,
            match_version: matched.map(|_| seeded.old),
            match_start: matched,
        };
        lock(&fixture.state.database)
            .unwrap()
            .confirm_change(&Confirmation {
                change_id: seeded.change,
                new_version_id: seeded.new,
                checks: "[{\"name\":\"length\"}]".to_string(),
                boundaries: "[9150.0]".to_string(),
                regions: vec![
                    region("existing", 9_000.0, 9_100.0, Some(6_400.0)),
                    region("new", 9_100.0, 9_200.0, None),
                    region("deleted", 6_700.0, 6_760.0, None),
                ],
            })
            .unwrap();
    }

    async fn post_empty(fixture: &Fixture, uri: &str) -> (StatusCode, Value) {
        send(fixture, Request::post(uri).body(Body::empty()).unwrap()).await
    }

    fn clip_body(change: i64) -> Value {
        json!({ "change": change, "kind": "addition", "start": 9_100.0, "end": 9_200.0 })
    }

    #[tokio::test]
    async fn a_confirmed_change_shows_both_bars() {
        let fixture = fixture();
        let seeded = seed(&fixture);
        confirm(&fixture, &seeded);

        let (status, body) =
            get_json(&fixture, &format!("/api/v1/changes/{}/diff", seeded.change)).await;

        assert_eq!(status, StatusCode::OK);
        let data = &body["data"];
        assert_eq!(data["state"], "confirmed");
        assert_eq!(data["old"]["version_id"], seeded.old);
        assert_eq!(data["old"]["start"], 6_340.0);
        let old_classes: Vec<&Value> = data["old"]["regions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|region| &region["class"])
            .collect();
        assert_eq!(old_classes, ["existing", "deleted"]);
        assert_eq!(data["new"]["start"], 9_000.0);
        assert_eq!(data["new"]["regions"].as_array().unwrap().len(), 2);
        assert_eq!(
            data["new"]["regions"][1]["video_boundaries"],
            json!([9_150.0])
        );
        assert_eq!(data["checks"][0]["name"], "length");
    }

    #[tokio::test]
    async fn a_provisional_change_shows_only_the_old_bar() {
        let fixture = fixture();
        let seeded = seed(&fixture);

        let (_, body) =
            get_json(&fixture, &format!("/api/v1/changes/{}/diff", seeded.change)).await;

        assert_eq!(body["data"]["old"]["version_id"], seeded.old);
        assert_eq!(body["data"]["new"], Value::Null);
        assert_eq!(body["data"]["checks"], json!([]));
    }

    #[tokio::test]
    async fn the_diff_of_an_unknown_change_is_not_found() {
        let fixture = fixture();

        let (status, body) = get_json(&fixture, "/api/v1/changes/9/diff").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn the_detail_of_a_change_has_region_ids_versions_and_clips() {
        let fixture = fixture();
        let seeded = seed(&fixture);
        confirm(&fixture, &seeded);
        send_body(&fixture, "POST", "/api/v1/clips", clip_body(seeded.change)).await;

        let (_, body) = get_json(&fixture, &format!("/api/v1/changes/{}", seeded.change)).await;

        let data = &body["data"];
        assert_eq!(data["old_version_id"], seeded.old);
        assert_eq!(data["new_version_id"], seeded.new);
        assert!(data["regions"][0]["id"].is_i64());
        assert_eq!(data["clips"][0]["state"], "draft");
    }

    #[tokio::test]
    async fn a_change_is_dismissed_once() {
        let fixture = fixture();
        let seeded = seed(&fixture);
        let uri = format!("/api/v1/changes/{}/dismiss", seeded.change);

        let (first, body) = post_empty(&fixture, &uri).await;
        let (second, error) = post_empty(&fixture, &uri).await;

        assert_eq!((first, second), (StatusCode::OK, StatusCode::CONFLICT));
        assert_eq!(body["data"]["state"], "dismissed");
        assert_eq!(error["error"]["code"], "invalid_request");
    }

    #[tokio::test]
    async fn dismissing_lets_the_holds_lapse() {
        let fixture = fixture();
        let seeded = seed(&fixture);
        let hold = lock(&fixture.state.database)
            .unwrap()
            .insert_hold(&NewHold {
                stream_id: fixture.stream,
                itag: 140,
                sq_first: 1,
                sq_last: 2,
                owner: HoldOwner::Change(seeded.change),
            })
            .unwrap();

        post_empty(
            &fixture,
            &format!("/api/v1/changes/{}/dismiss", seeded.change),
        )
        .await;

        let expires: Option<i64> = lock(&fixture.state.database)
            .unwrap()
            .connection()
            .query_row(
                "SELECT expires_at FROM retention_holds WHERE id = ?1",
                [hold],
                |row| row.get(0),
            )
            .unwrap();
        assert!(expires.is_some());
    }

    #[tokio::test]
    async fn dismissing_an_unknown_change_is_not_found() {
        let fixture = fixture();

        let (status, _) = post_empty(&fixture, "/api/v1/changes/9/dismiss").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_start_with_no_stored_audio_is_refused() {
        let fixture = fixture();
        let seeded = seed(&fixture);
        let body = json!({ "change": seeded.change, "kind": "cycle", "start": 2.0,
                           "end": 400.0 });

        let (status, error) = send_body(&fixture, "POST", "/api/v1/clips", body).await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["error"]["code"], "invalid_request");
    }

    #[tokio::test]
    async fn a_start_inside_a_fragment_is_stored_at_its_start() {
        let fixture = fixture();
        let seeded = seed(&fixture);
        let body = json!({ "change": seeded.change, "kind": "cycle", "start": 102.5,
                           "end": 400.0 });

        let (_, created) = send_body(&fixture, "POST", "/api/v1/clips", body).await;

        assert_eq!(created["data"]["start"], 100.0);
        assert_eq!(created["data"]["end"], 400.0);
    }

    #[tokio::test]
    async fn a_clip_is_made_read_and_edited() {
        let fixture = fixture();
        let seeded = seed(&fixture);
        let body = json!({ "change": seeded.change, "kind": "cycle", "start": 100.0,
                           "end": 400.0, "version": seeded.old });

        let (status, created) = send_body(&fixture, "POST", "/api/v1/clips", body).await;
        let id = created["data"]["id"].as_i64().unwrap();
        let (_, read) = get_json(&fixture, &format!("/api/v1/clips/{id}")).await;
        let (edited, patched) = send_body(
            &fixture,
            "PATCH",
            &format!("/api/v1/clips/{id}"),
            json!({ "kind": "addition", "end": 500.0 }),
        )
        .await;

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(created["data"]["state"], "draft");
        assert_eq!(created["data"]["stream_id"], fixture.stream);
        assert_eq!(created["data"]["version_id"], seeded.old);
        assert_eq!(read["data"], created["data"]);
        assert_eq!(edited, StatusCode::OK);
        assert_eq!(patched["data"]["kind"], "addition");
        assert_eq!(patched["data"]["start"], 100.0);
        assert_eq!(patched["data"]["end"], 500.0);
    }

    #[tokio::test]
    async fn a_clip_that_makes_no_cut_is_a_bad_request() {
        let fixture = fixture();
        let seeded = seed(&fixture);
        let bodies = [
            json!({ "change": seeded.change, "kind": "addition", "start": 5.0, "end": 5.0 }),
            json!({ "change": seeded.change, "kind": "addition", "start": 0.0,
                    "end": 700_000.0 }),
            json!({ "change": seeded.change, "kind": "addition", "start": 0.0, "end": 5.0,
                    "colour": "red" }),
            json!({ "change": seeded.change, "kind": "movie", "start": 0.0, "end": 5.0 }),
        ];

        for body in bodies {
            let (status, error) = send_body(&fixture, "POST", "/api/v1/clips", body.clone()).await;

            assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(error["error"]["code"], "invalid_request");
        }
    }

    #[tokio::test]
    async fn an_empty_edit_is_a_bad_request() {
        let fixture = fixture();
        let seeded = seed(&fixture);
        let (_, created) =
            send_body(&fixture, "POST", "/api/v1/clips", clip_body(seeded.change)).await;
        let id = created["data"]["id"].as_i64().unwrap();

        let (status, _) =
            send_body(&fixture, "PATCH", &format!("/api/v1/clips/{id}"), json!({})).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_clip_of_an_unknown_change_is_not_found() {
        let fixture = fixture();

        let (status, _) = send_body(&fixture, "POST", "/api/v1/clips", clip_body(9)).await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn an_unknown_clip_is_not_found() {
        let fixture = fixture();

        let (read, _) = get_json(&fixture, "/api/v1/clips/9").await;
        let (edited, _) = send_body(
            &fixture,
            "PATCH",
            "/api/v1/clips/9",
            json!({ "kind": "cycle" }),
        )
        .await;

        assert_eq!(
            (read, edited),
            (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND)
        );
    }

    #[tokio::test]
    async fn a_version_from_elsewhere_is_a_conflict() {
        let fixture = fixture();
        let seeded = seed(&fixture);
        let mut body = clip_body(seeded.change);
        body["version"] = json!(seeded.new + 100);

        let (status, error) = send_body(&fixture, "POST", "/api/v1/clips", body).await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["error"]["code"], "invalid_request");
    }

    #[tokio::test]
    async fn an_exported_clip_cannot_be_edited() {
        let fixture = fixture();
        let seeded = seed(&fixture);
        let (_, created) =
            send_body(&fixture, "POST", "/api/v1/clips", clip_body(seeded.change)).await;
        let id = created["data"]["id"].as_i64().unwrap();
        lock(&fixture.state.database)
            .unwrap()
            .connection()
            .execute(
                "UPDATE clips SET state = 'exported', name = 'Example', video_itag = 248,
                 audio_itag = 140, sq_first = 1, sq_last = 2, duration = 10.0, metadata = '{}'",
                [],
            )
            .unwrap();

        let (status, error) = send_body(
            &fixture,
            "PATCH",
            &format!("/api/v1/clips/{id}"),
            json!({ "end": 300.0 }),
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(error["error"]["message"], "the clip has been exported");
    }
}

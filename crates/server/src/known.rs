//! Known material of a program, stored for analysis to leave out from M7 on (FR-K1).

use crate::api::{ApiError, ApiState, Envelope, ok, store_error, with_database};
use crate::catalog::program_with_streams;
use crate::operations::{invalid, parse_body};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use dalsegno_app::catalog::KnownStretch;
use dalsegno_store::known::KnownClipRow;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::task::spawn_blocking;

/// Longest clip, in seconds. Recurring clips are short, and a longer stretch is a mistake.
const MAX_CLIP_SECONDS: f64 = 1_800.0;
/// Longest name, in characters.
const MAX_NAME_CHARS: usize = 200;

/// A stretch of a stream to keep, in stream time.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KnownClipBody {
    name: String,
    stream: i64,
    start: f64,
    end: f64,
}

fn clip_json(clip: &KnownClipRow) -> Value {
    json!({
        "id": clip.id,
        "name": clip.name,
        "duration": clip.duration,
        "feature": clip.feature,
    })
}

/// Lists a program's known clips, which are few, so all of them.
pub(crate) async fn list_known_clips(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        program_with_streams(database, id)?;
        let clips = database.known_clips(id).map_err(store_error)?;
        Ok(ok(clips.iter().map(clip_json).collect()))
    })
    .await
}

/// Cuts a stretch of a stream's envelope into known material.
pub(crate) async fn add_known_clip(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<(StatusCode, Json<Envelope>), ApiError> {
    let request: KnownClipBody = parse_body(&body)?;
    let name = request.name.trim().to_string();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(invalid("the name must be from 1 to 200 characters"));
    }
    let length = request.end - request.start;
    if request.start < 0.0 || !(length > 0.0 && length <= MAX_CLIP_SECONDS) {
        return Err(invalid(
            "the stretch must start at 0 or later and last more than 0 and at most 1800 seconds",
        ));
    }
    let stream = request.stream;
    with_database(&state.database, move |database| {
        program_with_streams(database, id)?;
        database
            .stream(stream)
            .map_err(store_error)?
            .ok_or_else(ApiError::not_found)?;
        Ok(())
    })
    .await?;
    let library = state.library.clone();
    let clip = spawn_blocking(move || {
        library.add_known_clip(
            id,
            &KnownStretch {
                name: &name,
                stream_id: stream,
                start: request.start,
                end: request.end,
            },
        )
    })
    .await
    .map_err(|error| ApiError::internal(&error.to_string()))??;
    Ok((StatusCode::CREATED, ok(clip_json(&clip))))
}

/// Removes a known clip of a program.
pub(crate) async fn remove_known_clip(
    State(state): State<ApiState>,
    Path((id, clip)): Path<(i64, i64)>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        program_with_streams(database, id)?;
        if !database.delete_known_clip(id, clip).map_err(store_error)? {
            return Err(ApiError::not_found());
        }
        Ok(ok(json!({ "id": clip })))
    })
    .await
}

#[cfg(test)]
mod tests {
    use crate::api::tests::{Fixture, fixture, get_json, send_body};
    use axum::http::StatusCode;
    use dalsegno_app::fetch::lock;
    use dalsegno_store::features::EnvelopeRow;
    use serde_json::{Value, json};

    async fn program(fixture: &Fixture) -> i64 {
        let (_, body) = send_body(
            fixture,
            "POST",
            "/api/v1/programs",
            json!({ "short_name": "Example TV", "output_dir": "Example TV" }),
        )
        .await;
        body["data"]["id"].as_i64().unwrap()
    }

    /// Ten seconds of envelope from the start, in one fragment.
    fn envelope(fixture: &Fixture) {
        let mut database = lock(&fixture.state.database).unwrap();
        let run = database.insert_feature_run(fixture.stream, 1, 0.0).unwrap();
        let row = EnvelopeRow {
            sq: 1,
            first_bin: 0,
            values: vec![0.5; 100],
        };
        database.advance_feature_run(&run, &row).unwrap();
    }

    fn clip_body(fixture: &Fixture, start: f64, end: f64) -> Value {
        json!({ "name": " Opening ", "stream": fixture.stream, "start": start, "end": end })
    }

    #[tokio::test]
    async fn a_clip_is_added_listed_and_removed() {
        let fixture = fixture();
        envelope(&fixture);
        let program = program(&fixture).await;
        let uri = format!("/api/v1/programs/{program}/known-clips");

        let (status, added) =
            send_body(&fixture, "POST", &uri, clip_body(&fixture, 2.0, 5.0)).await;
        let (_, listed) = get_json(&fixture, &uri).await;
        let clip = added["data"]["id"].as_i64().unwrap();
        let (removed_status, removed) =
            send_body(&fixture, "DELETE", &format!("{uri}/{clip}"), json!(null)).await;
        let (_, after) = get_json(&fixture, &uri).await;

        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(added["data"]["name"], "Opening");
        assert_eq!(added["data"]["duration"], 3.0);
        assert_eq!(added["data"]["feature"], "env10");
        assert_eq!(listed["data"], json!([added["data"]]));
        assert_eq!(removed_status, StatusCode::OK);
        assert_eq!(removed["data"], json!({ "id": clip }));
        assert_eq!(after["data"], json!([]));
    }

    #[tokio::test]
    async fn a_clip_the_envelope_does_not_cover_is_a_conflict() {
        let fixture = fixture();
        envelope(&fixture);
        let program = program(&fixture).await;

        let (status, body) = send_body(
            &fixture,
            "POST",
            &format!("/api/v1/programs/{program}/known-clips"),
            clip_body(&fixture, 8.0, 12.0),
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(
            body["error"]["message"],
            "the stored envelope does not cover that stretch"
        );
    }

    #[tokio::test]
    async fn a_clip_that_does_not_fit_is_refused() {
        let fixture = fixture();
        let program = program(&fixture).await;
        let uri = format!("/api/v1/programs/{program}/known-clips");
        let cases = [
            clip_body(&fixture, 5.0, 5.0),
            clip_body(&fixture, 5.0, 4.0),
            clip_body(&fixture, 0.0, 1801.0),
            clip_body(&fixture, -1.0, 2.0),
            json!({ "name": "  ", "stream": fixture.stream, "start": 0.0, "end": 2.0 }),
            json!({ "name": "x", "stream": fixture.stream, "start": 0.0 }),
        ];

        for request in cases {
            let (status, _) = send_body(&fixture, "POST", &uri, request.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{request}");
        }
    }

    #[tokio::test]
    async fn clips_need_a_known_program_and_stream() {
        let fixture = fixture();
        let program = program(&fixture).await;

        let (no_program, _) = send_body(
            &fixture,
            "POST",
            "/api/v1/programs/99/known-clips",
            clip_body(&fixture, 0.0, 2.0),
        )
        .await;
        let (no_stream, _) = send_body(
            &fixture,
            "POST",
            &format!("/api/v1/programs/{program}/known-clips"),
            json!({ "name": "x", "stream": 99, "start": 0.0, "end": 2.0 }),
        )
        .await;
        let (listing, _) = get_json(&fixture, "/api/v1/programs/99/known-clips").await;

        assert_eq!(no_program, StatusCode::NOT_FOUND);
        assert_eq!(no_stream, StatusCode::NOT_FOUND);
        assert_eq!(listing, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_clip_of_another_program_is_not_removed() {
        let fixture = fixture();
        envelope(&fixture);
        let owner = program(&fixture).await;
        let other = program(&fixture).await;
        let (_, added) = send_body(
            &fixture,
            "POST",
            &format!("/api/v1/programs/{owner}/known-clips"),
            clip_body(&fixture, 2.0, 5.0),
        )
        .await;
        let clip = added["data"]["id"].as_i64().unwrap();

        let (status, _) = send_body(
            &fixture,
            "DELETE",
            &format!("/api/v1/programs/{other}/known-clips/{clip}"),
            json!(null),
        )
        .await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}

//! Streams and programs managed by hand (FR-S1, FR-S3).

use crate::api::{
    ApiError, ApiState, Envelope, Paging, change_json, ok, store_error, stream_json, with_database,
};
use crate::operations::{invalid, parse_body};
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use dalsegno_app::acquisition::video_id_of;
use dalsegno_app::catalog::check_program;
use dalsegno_app::error::AcquireError;
use dalsegno_app::export::{DEFAULT_SUFFIX, DEFAULT_TEMPLATE, NewProgram};
use dalsegno_store::database::Database;
use dalsegno_store::programs::ProgramRow;
use dalsegno_store::status::{Listing, VersionStatus};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::task::spawn_blocking;

/// Longest stream title, in characters.
const MAX_TITLE_CHARS: usize = 300;

/// A stream to register, by video ID or URL.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AddStreamBody {
    stream: String,
}

/// A new title for a stream.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TitleBody {
    title: String,
}

/// The program a stream is to join.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkBody {
    program: i64,
}

/// What a program is, as `POST` and `PUT` take it. The channel is never set by hand.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProgramBody {
    short_name: String,
    output_dir: String,
    name_template: Option<String>,
    addition_suffix: Option<String>,
    artist: Option<String>,
    content_date: Option<String>,
    description: Option<String>,
}

/// Text that is empty once trimmed is no text.
fn optional_text(text: Option<String>) -> Option<String> {
    text.map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

impl ProgramBody {
    /// Reads the body into a program that can name its output, keeping `channel_id`.
    fn into_program(self, channel_id: Option<String>) -> Result<NewProgram, ApiError> {
        let program = NewProgram {
            short_name: self.short_name.trim().to_string(),
            channel_id,
            name_template: self
                .name_template
                .unwrap_or_else(|| DEFAULT_TEMPLATE.to_string()),
            addition_suffix: self
                .addition_suffix
                .unwrap_or_else(|| DEFAULT_SUFFIX.to_string()),
            artist: optional_text(self.artist),
            content_date: optional_text(self.content_date),
            description: optional_text(self.description),
            output_dir: self.output_dir.trim().to_string(),
        };
        check_program(&program).map_err(|error| invalid(&error.to_string()))?;
        Ok(program)
    }
}

pub(crate) fn program_json(program: &ProgramRow, stream_ids: &[i64]) -> Value {
    json!({
        "id": program.id,
        "short_name": program.program.short_name,
        "channel_id": program.program.channel_id,
        "name_template": program.program.name_template,
        "addition_suffix": program.program.addition_suffix,
        "artist": program.program.artist,
        "content_date": program.program.content_date,
        "description": program.program.description,
        "output_dir": program.program.output_dir,
        "stream_ids": stream_ids,
    })
}

fn version_json(version: &VersionStatus) -> Value {
    json!({
        "id": version.id,
        "stream_id": version.stream_id,
        "period_audio": version.period_audio,
        "period_video": version.period_video,
        "anchor_time": version.anchor_time,
        "first_seen_at": version.first_seen_at,
        "on_air_from": version.on_air_from,
        "on_air_until": version.on_air_until,
        "last_cycle_start": version.last_cycle_start,
    })
}

/// Reads a program with its streams, or answers that there is none.
pub(crate) fn program_with_streams(
    database: &Database,
    id: i64,
) -> Result<(ProgramRow, Vec<i64>), ApiError> {
    let program = database
        .program(id)
        .map_err(store_error)?
        .ok_or_else(ApiError::not_found)?;
    let streams = database.program_stream_ids(id).map_err(store_error)?;
    Ok((program, streams))
}

fn stream_body(database: &Database, id: i64) -> Result<Value, ApiError> {
    let stream = database.stream_status(id).map_err(store_error)?;
    Ok(stream_json(&stream.ok_or_else(ApiError::not_found)?))
}

/// Registers a stream, which waits for the probe and so can take minutes.
pub(crate) async fn add_stream(
    State(state): State<ApiState>,
    body: Bytes,
) -> Result<(StatusCode, Json<Envelope>), ApiError> {
    let request: AddStreamBody = parse_body(&body)?;
    let video = video_id_of(&request.stream).map_err(|error| match error {
        AcquireError::InvalidRequest(message) => invalid(message),
        other => ApiError::from(other),
    })?;
    let lookup = video.clone();
    let is_known = with_database(&state.database, move |database| {
        Ok(database
            .stream_by_video_id(&lookup)
            .map_err(store_error)?
            .is_some())
    })
    .await?;
    if is_known {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "already_registered",
            "the stream is already registered",
        ));
    }
    let report = state
        .services
        .acquisition
        .add(&state.library, &video)
        .await
        .map_err(|error| match error {
            AcquireError::NoAudio(_) => ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "no_audio",
                &error.to_string(),
            ),
            other => {
                // The reason can quote the origin, so it stays in the log.
                tracing::warn!(error = %other, "the probe failed");
                ApiError::new(
                    StatusCode::BAD_GATEWAY,
                    "probe_failed",
                    "the probe failed, see the service log",
                )
            }
        })?;
    let (stream_id, probe) = (report.stream_id, report.probe);
    let stream = with_database(&state.database, move |database| {
        stream_body(database, stream_id)
    })
    .await?;
    Ok((
        StatusCode::CREATED,
        ok(json!({ "stream": stream, "probe": probe })),
    ))
}

/// Changes a stream's title.
pub(crate) async fn patch_stream(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<Json<Envelope>, ApiError> {
    let request: TitleBody = parse_body(&body)?;
    let title = request.title.trim().to_string();
    if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
        return Err(invalid("the title must be from 1 to 300 characters"));
    }
    with_database(&state.database, move |database| {
        database
            .stream(id)
            .map_err(store_error)?
            .ok_or_else(ApiError::not_found)?;
        database.set_stream_title(id, &title).map_err(store_error)?;
        stream_body(database, id).map(ok)
    })
    .await
}

/// Makes a stream part of a program, unless that would strand versions.
pub(crate) async fn link_stream(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<Json<Envelope>, ApiError> {
    let request: LinkBody = parse_body(&body)?;
    with_database(&state.database, move |database| {
        database
            .stream(id)
            .map_err(store_error)?
            .ok_or_else(ApiError::not_found)?;
        database
            .program(request.program)
            .map_err(store_error)?
            .ok_or_else(ApiError::not_found)?;
        Ok(())
    })
    .await?;
    let library = state.library.clone();
    spawn_blocking(move || library.link_stream(id, request.program))
        .await
        .map_err(|error| ApiError::internal(&error.to_string()))??;
    with_database(&state.database, move |database| {
        stream_body(database, id).map(ok)
    })
    .await
}

/// Lists programs, oldest first.
pub(crate) async fn list_programs(
    State(state): State<ApiState>,
    Query(paging): Query<Paging>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        let total = database
            .count_rows(Listing::Programs)
            .map_err(store_error)?;
        let rows = database
            .list_programs(paging.limit(), paging.offset())
            .map_err(store_error)?;
        let items = rows
            .iter()
            .map(|row| {
                let streams = database.program_stream_ids(row.id).map_err(store_error)?;
                Ok(program_json(row, &streams))
            })
            .collect::<Result<_, ApiError>>()?;
        Ok(paging.envelope(total, items))
    })
    .await
}

/// Makes a program, which no stream is linked to yet.
pub(crate) async fn create_program(
    State(state): State<ApiState>,
    body: Bytes,
) -> Result<(StatusCode, Json<Envelope>), ApiError> {
    let program = parse_body::<ProgramBody>(&body)?.into_program(None)?;
    let created = with_database(&state.database, move |database| {
        let id = database.insert_program(&program).map_err(store_error)?;
        Ok(program_json(&ProgramRow { id, program }, &[]))
    })
    .await?;
    Ok((StatusCode::CREATED, ok(created)))
}

/// Shows a program.
pub(crate) async fn program_detail(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        let (program, streams) = program_with_streams(database, id)?;
        Ok(ok(program_json(&program, &streams)))
    })
    .await
}

/// Replaces a program's values, which keeps its versions and its channel.
pub(crate) async fn put_program(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
    body: Bytes,
) -> Result<Json<Envelope>, ApiError> {
    let request: ProgramBody = parse_body(&body)?;
    with_database(&state.database, move |database| {
        let (stored, streams) = program_with_streams(database, id)?;
        let program = request.into_program(stored.program.channel_id)?;
        database.update_program(id, &program).map_err(store_error)?;
        Ok(ok(program_json(&ProgramRow { id, program }, &streams)))
    })
    .await
}

/// Lists a program's versions, newest first.
pub(crate) async fn list_versions(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
    Query(paging): Query<Paging>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        program_with_streams(database, id)?;
        let total = database.count_versions(id).map_err(store_error)?;
        let rows = database
            .list_versions(id, paging.limit(), paging.offset())
            .map_err(store_error)?;
        Ok(paging.envelope(total, rows.iter().map(version_json).collect()))
    })
    .await
}

/// Lists a program's changes, newest first.
pub(crate) async fn list_program_changes(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
    Query(paging): Query<Paging>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        program_with_streams(database, id)?;
        let total = database.count_program_changes(id).map_err(store_error)?;
        let rows = database
            .list_program_changes(id, paging.limit(), paging.offset())
            .map_err(store_error)?;
        Ok(paging.envelope(total, rows.iter().map(change_json).collect()))
    })
    .await
}

#[cfg(test)]
mod tests {
    use crate::api::tests::{Fixture, fixture, get_json, send_body};
    use axum::http::StatusCode;
    use dalsegno_app::fetch::lock;
    use dalsegno_store::changes::NewVersion;
    use serde_json::{Value, json};

    fn program_body() -> Value {
        json!({ "short_name": "Example TV", "output_dir": "Example TV" })
    }

    async fn made_program(fixture: &Fixture) -> i64 {
        let (status, body) = send_body(fixture, "POST", "/api/v1/programs", program_body()).await;
        assert_eq!(status, StatusCode::CREATED);
        body["data"]["id"].as_i64().unwrap()
    }

    #[tokio::test]
    async fn a_stream_shows_the_program_it_is_linked_to() {
        let fixture = fixture();
        let uri = format!("/api/v1/streams/{}", fixture.stream);
        let program = made_program(&fixture).await;
        let (_, before) = get_json(&fixture, &uri).await;

        send_body(
            &fixture,
            "POST",
            &format!("{uri}/link"),
            json!({ "program": program }),
        )
        .await;
        let (_, after) = get_json(&fixture, &uri).await;

        assert_eq!(before["data"]["program_id"], Value::Null);
        assert_eq!(after["data"]["program_id"], program);
    }

    #[tokio::test]
    async fn a_stream_that_cannot_be_added_is_refused() {
        let fixture = fixture();
        let cases = [
            (
                json!({ "stream": "not a video" }),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                json!({ "stream": "abcDEF123_-", "extra": 1 }),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                json!({ "stream": "abcDEF123_-" }),
                StatusCode::CONFLICT,
                "already_registered",
            ),
            (
                json!({ "stream": "https://www.youtube.com/watch?v=abcDEF123_-" }),
                StatusCode::CONFLICT,
                "already_registered",
            ),
        ];

        for (request, expected, code) in cases {
            let (status, body) =
                send_body(&fixture, "POST", "/api/v1/streams", request.clone()).await;
            assert_eq!(status, expected, "{request}");
            assert_eq!(body["error"]["code"], code, "{request}");
        }
        let (_, streams) = get_json(&fixture, "/api/v1/streams").await;
        assert_eq!(streams["meta"]["total"], 1);
    }

    #[tokio::test]
    async fn a_title_is_trimmed_and_stored() {
        let fixture = fixture();
        let uri = format!("/api/v1/streams/{}", fixture.stream);

        let (status, body) =
            send_body(&fixture, "PATCH", &uri, json!({ "title": "  Renamed  " })).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["title"], "Renamed");
        let (_, stored) = get_json(&fixture, &uri).await;
        assert_eq!(stored["data"]["title"], "Renamed");
    }

    #[tokio::test]
    async fn a_title_that_does_not_fit_is_refused() {
        let fixture = fixture();
        let uri = format!("/api/v1/streams/{}", fixture.stream);
        let too_long = "t".repeat(301);

        for title in ["   ", too_long.as_str()] {
            let (status, _) = send_body(&fixture, "PATCH", &uri, json!({ "title": title })).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
        }
        let (unknown, _) = send_body(
            &fixture,
            "PATCH",
            "/api/v1/streams/99",
            json!({ "title": "x" }),
        )
        .await;
        assert_eq!(unknown, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_program_is_made_with_defaults_and_no_channel() {
        let fixture = fixture();

        let (status, body) = send_body(
            &fixture,
            "POST",
            "/api/v1/programs",
            json!({ "short_name": " Example TV ", "output_dir": "Example TV", "artist": "  " }),
        )
        .await;

        assert_eq!(status, StatusCode::CREATED);
        let data = &body["data"];
        assert_eq!(data["short_name"], "Example TV");
        assert_eq!(data["name_template"], "{name} {datetime}{suffix} ({year})");
        assert_eq!(data["addition_suffix"], "追加分");
        assert_eq!(data["artist"], Value::Null);
        assert_eq!(data["channel_id"], Value::Null);
        assert_eq!(data["stream_ids"], json!([]));
    }

    #[tokio::test]
    async fn a_program_that_cannot_name_its_output_is_refused() {
        let fixture = fixture();
        let cases = [
            json!({ "short_name": "  ", "output_dir": "Example TV" }),
            json!({ "short_name": "Example TV", "output_dir": "/etc" }),
            json!({ "short_name": "Example TV", "output_dir": "x", "name_template": "{oops}" }),
            json!({ "short_name": "Example TV" }),
            json!({ "short_name": "Example TV", "output_dir": "x", "channel_id": "UC" }),
        ];

        for request in cases {
            let (status, body) =
                send_body(&fixture, "POST", "/api/v1/programs", request.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{request}");
            assert_eq!(body["error"]["code"], "invalid_request");
        }
        let (_, programs) = get_json(&fixture, "/api/v1/programs").await;
        assert_eq!(programs["meta"]["total"], 0);
    }

    #[tokio::test]
    async fn an_unknown_placeholder_is_named_in_the_refusal() {
        let fixture = fixture();

        let (_, body) = send_body(
            &fixture,
            "POST",
            "/api/v1/programs",
            json!({ "short_name": "Example TV", "output_dir": "x", "name_template": "{oops}" }),
        )
        .await;

        assert_eq!(
            body["error"]["message"],
            "unknown placeholder {oops} in the name template"
        );
    }

    #[tokio::test]
    async fn a_program_is_replaced_whole_and_keeps_its_streams() {
        let fixture = fixture();
        let program = made_program(&fixture).await;
        send_body(
            &fixture,
            "POST",
            &format!("/api/v1/streams/{}/link", fixture.stream),
            json!({ "program": program }),
        )
        .await;
        let uri = format!("/api/v1/programs/{program}");

        let (status, body) = send_body(
            &fixture,
            "PUT",
            &uri,
            json!({ "short_name": "Renamed", "output_dir": "Renamed", "artist": "Example" }),
        )
        .await;
        let (_, stored) = get_json(&fixture, &uri).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"], stored["data"]);
        assert_eq!(stored["data"]["short_name"], "Renamed");
        assert_eq!(stored["data"]["artist"], "Example");
        assert_eq!(stored["data"]["stream_ids"], json!([fixture.stream]));
        let (missing, _) = send_body(&fixture, "PUT", "/api/v1/programs/99", program_body()).await;
        assert_eq!(missing, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn programs_are_listed_oldest_first() {
        let fixture = fixture();
        let first = made_program(&fixture).await;
        let second = made_program(&fixture).await;

        let (_, body) = get_json(&fixture, "/api/v1/programs").await;
        let (status, _) = get_json(&fixture, "/api/v1/programs/99").await;

        assert_eq!(body["meta"]["total"], 2);
        assert_eq!(body["data"][0]["id"], first);
        assert_eq!(body["data"][1]["id"], second);
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn linking_needs_a_stream_and_a_program() {
        let fixture = fixture();
        let program = made_program(&fixture).await;

        let (no_stream, _) = send_body(
            &fixture,
            "POST",
            "/api/v1/streams/99/link",
            json!({ "program": program }),
        )
        .await;
        let (no_program, _) = send_body(
            &fixture,
            "POST",
            &format!("/api/v1/streams/{}/link", fixture.stream),
            json!({ "program": 99 }),
        )
        .await;

        assert_eq!(no_stream, StatusCode::NOT_FOUND);
        assert_eq!(no_program, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_program_with_versions_from_another_stream_is_refused_with_409() {
        let fixture = fixture();
        let program = made_program(&fixture).await;
        {
            let database = lock(&fixture.state.database).unwrap();
            let other = database
                .insert_stream(&dalsegno_store::streams::NewStream {
                    video_id: "zzzZZZ999_-".to_string(),
                    title: "Other".to_string(),
                    channel_id: None,
                    channel_name: None,
                    thumbnail_url: None,
                    video_itag: None,
                    audio_itag: 140,
                    fragment_len: 5.0,
                    registered_at: 0,
                })
                .unwrap();
            database
                .insert_version(&NewVersion {
                    program_id: program,
                    stream_id: other,
                    period: 100.0,
                    anchor_time: 0.0,
                    seen_at: 0,
                    on_air_from: 0.0,
                })
                .unwrap();
        }

        let (status, body) = send_body(
            &fixture,
            "POST",
            &format!("/api/v1/streams/{}/link", fixture.stream),
            json!({ "program": program }),
        )
        .await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "invalid_request");
    }

    #[tokio::test]
    async fn versions_and_changes_list_by_program() {
        let fixture = fixture();
        let program = made_program(&fixture).await;
        lock(&fixture.state.database)
            .unwrap()
            .insert_version(&NewVersion {
                program_id: program,
                stream_id: fixture.stream,
                period: 100.0,
                anchor_time: 5.0,
                seen_at: 9,
                on_air_from: 1.0,
            })
            .unwrap();

        let (status, versions) =
            get_json(&fixture, &format!("/api/v1/programs/{program}/versions")).await;
        let (_, changes) = get_json(&fixture, &format!("/api/v1/programs/{program}/changes")).await;
        let (missing, _) = get_json(&fixture, "/api/v1/programs/99/versions").await;
        let (missing_changes, _) = get_json(&fixture, "/api/v1/programs/99/changes").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(versions["meta"]["total"], 1);
        assert_eq!(versions["data"][0]["stream_id"], fixture.stream);
        assert_eq!(versions["data"][0]["period_audio"], 100.0);
        assert_eq!(versions["data"][0]["on_air_until"], Value::Null);
        assert_eq!(changes["meta"]["total"], 0);
        assert_eq!(missing, StatusCode::NOT_FOUND);
        assert_eq!(missing_changes, StatusCode::NOT_FOUND);
    }
}

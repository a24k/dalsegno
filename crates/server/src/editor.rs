//! The boundary editor's data: candidates, a preview to play and a contact sheet for each edge
//! of a draft clip (FR-E2, FR-E4).

use crate::api::{ApiError, ApiState, Envelope, job_json, ok};
use crate::changes::clip_json;
use crate::operations::invalid;
use crate::runner::now_ms;
use axum::Json;
use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use dalsegno_app::acquisition::Library;
use dalsegno_app::error::AcquireError;
use dalsegno_app::export::{CANDIDATES, CandidatesTarget, Edge, EdgeFile, editor_state};
use dalsegno_app::fetch::lock;
use dalsegno_store::jobs::JobRow;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::task::spawn_blocking;
use tower_http::services::ServeFile;

/// Which end of the cut a file is for.
#[derive(Debug, Deserialize)]
pub(crate) struct EdgeQuery {
    edge: Option<String>,
}

fn parse_edge(query: &EdgeQuery) -> Result<Edge, ApiError> {
    query
        .edge
        .as_deref()
        .and_then(Edge::parse)
        .ok_or_else(|| invalid("the edge must be start or end"))
}

fn to_value<T: serde::Serialize>(value: &T) -> Result<Value, ApiError> {
    serde_json::to_value(value).map_err(|error| ApiError::internal(&error.to_string()))
}

/// Returns the job that makes a clip's candidates, queuing it unless one is waiting or running.
fn queued(library: &Library, clip: i64) -> Result<JobRow, ApiError> {
    let job = CandidatesTarget { clip }.job()?;
    let shared = library.database();
    let database = lock(&shared).map_err(|error| ApiError::internal(&error.to_string()))?;
    database
        .enqueue_job_once(&job, now_ms())
        .map_err(|error| ApiError::internal(&error.to_string()))?;
    database
        .active_job(CANDIDATES, &job.target)
        .map_err(|error| ApiError::internal(&error.to_string()))?
        .ok_or_else(|| ApiError::internal("the queued job is gone"))
}

/// Returns what the editor shows for a clip. A draft that lacks something gets the job queued
/// to make it, and the answer is 202 with the job.
pub(crate) async fn clip_candidates(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<(StatusCode, Json<Envelope>), ApiError> {
    let library = state.library.clone();
    spawn_blocking(move || {
        let editor = editor_state(&library, id)?;
        let job = editor.needs_job.then(|| queued(&library, id)).transpose()?;
        let status = if job.is_some() {
            StatusCode::ACCEPTED
        } else {
            StatusCode::OK
        };
        let body = json!({
            "clip": clip_json(&editor.clip),
            "start": to_value(&editor.start)?,
            "end": to_value(&editor.end)?,
            "period": to_value(&editor.period)?,
            "job": job.as_ref().map(job_json),
        });
        Ok((status, ok(body)))
    })
    .await
    .map_err(|error| ApiError::internal(&error.to_string()))?
}

/// Serves one of an edge's current files, with ranges for the player.
async fn serve_edge(
    state: ApiState,
    id: i64,
    query: EdgeQuery,
    file: EdgeFile,
    request: Request,
) -> Result<Response, ApiError> {
    let edge = parse_edge(&query)?;
    let library = state.library.clone();
    let path = spawn_blocking(move || library.current_edge_file(id, edge, file))
        .await
        .map_err(|error| ApiError::internal(&error.to_string()))?
        .map_err(|error| match error {
            AcquireError::NotFound(_) => ApiError::not_found(),
            other => other.into(),
        })?;
    let response = ServeFile::new(path)
        .ignore_multi_range_requests(true)
        .try_call(request)
        .await
        .map_err(|error| ApiError::internal(&error.to_string()))?;
    Ok(response.map(Body::new).into_response())
}

/// Serves an edge's preview video.
pub(crate) async fn clip_preview(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
    Query(query): Query<EdgeQuery>,
    request: Request,
) -> Result<Response, ApiError> {
    let mut response = serve_edge(state, id, query, EdgeFile::Preview, request).await?;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    Ok(response)
}

/// Serves an edge's contact sheet.
pub(crate) async fn clip_sheet(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
    Query(query): Query<EdgeQuery>,
    request: Request,
) -> Result<Response, ApiError> {
    serve_edge(state, id, query, EdgeFile::Sheet, request).await
}

#[cfg(test)]
mod tests {
    use crate::api::router;
    use crate::api::tests::{Fixture, fixture, get_json};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use axum::response::Response;
    use dalsegno_app::export::{Edge, EdgeReport, Window};
    use dalsegno_app::fetch::lock;
    use serde_json::Value;
    use std::fs;
    use tower::ServiceExt;

    const BODY_LIMIT: usize = 1 << 20;
    const PREVIEW_BYTES: usize = 1_000;

    fn bytes() -> Vec<u8> {
        (0..PREVIEW_BYTES)
            .map(|index| (index % 251) as u8)
            .collect()
    }

    fn insert_clip(fixture: &Fixture, state: &str, start: f64, end: f64) -> i64 {
        let database = lock(&fixture.state.database).unwrap();
        let extra = if state == "exported" {
            ", name, video_itag, audio_itag, sq_first, sq_last, duration, metadata"
        } else {
            ""
        };
        let values = if state == "exported" {
            ", 'Example', 248, 140, 1, 2, 10.0, '{}'"
        } else {
            ""
        };
        let stream = fixture.stream;
        database
            .connection()
            .execute(
                &format!(
                    "INSERT INTO clips (state, kind, stream_id, start_time, end_time{extra})
                     VALUES ('{state}', 'addition', {stream}, {start}, {end}{values})"
                ),
                [],
            )
            .unwrap();
        database.connection().last_insert_rowid()
    }

    /// Writes an edge's work files as the job leaves them, for a window of 1000 s to 1040 s.
    fn write_edge(fixture: &Fixture, clip: i64, edge: Edge) {
        let directory = fixture.state.library.edge_directory(clip, edge);
        fs::create_dir_all(&directory).unwrap();
        let report = EdgeReport {
            around: 1_020.0,
            window: Window {
                start: 1_000.0,
                end: 1_040.0,
            },
            preview_start: 1_000.0,
            keyframes: Vec::new(),
            black: Vec::new(),
            silence: Vec::new(),
            starts: Vec::new(),
            ends: Vec::new(),
            relaxed: false,
        };
        fs::write(
            directory.join("report.json"),
            serde_json::to_vec(&report).unwrap(),
        )
        .unwrap();
        fs::write(directory.join("preview.mp4"), bytes()).unwrap();
        fs::write(directory.join("sheet.jpg"), b"jpeg").unwrap();
    }

    fn job_count(fixture: &Fixture) -> i64 {
        lock(&fixture.state.database)
            .unwrap()
            .connection()
            .query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))
            .unwrap()
    }

    async fn raw(fixture: &Fixture, uri: &str, range: Option<&str>) -> Response {
        let mut request = Request::get(uri);
        if let Some(range) = range {
            request = request.header(header::RANGE, range);
        }
        router(fixture.state.clone())
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn body_of(response: Response) -> Vec<u8> {
        to_bytes(response.into_body(), BODY_LIMIT)
            .await
            .unwrap()
            .to_vec()
    }

    fn header_text(response: &Response, name: header::HeaderName) -> Option<String> {
        response
            .headers()
            .get(name)
            .map(|value| value.to_str().unwrap().to_string())
    }

    /// A draft with both edges ready, and its preview URL.
    fn ready(fixture: &Fixture) -> (i64, String) {
        let clip = insert_clip(fixture, "draft", 1_010.0, 1_030.0);
        write_edge(fixture, clip, Edge::Start);
        write_edge(fixture, clip, Edge::End);
        (clip, format!("/api/v1/clips/{clip}/preview?edge=start"))
    }

    #[tokio::test]
    async fn a_draft_that_lacks_reports_queues_one_job_however_often_it_is_asked() {
        let fixture = fixture();
        let clip = insert_clip(&fixture, "draft", 1_010.0, 1_030.0);
        let uri = format!("/api/v1/clips/{clip}/candidates");

        let (first, body) = get_json(&fixture, &uri).await;
        let (second, again) = get_json(&fixture, &uri).await;

        assert_eq!(
            (first, second),
            (StatusCode::ACCEPTED, StatusCode::ACCEPTED)
        );
        assert_eq!(job_count(&fixture), 1);
        let job = &body["data"]["job"];
        assert_eq!(
            (job["kind"].as_str(), job["state"].as_str()),
            (Some("candidates"), Some("queued"))
        );
        assert_eq!(job["target"]["clip"], clip);
        assert_eq!(again["data"]["job"]["id"], job["id"]);
        assert_eq!(body["data"]["clip"]["id"], clip);
        assert_eq!(body["data"]["start"], Value::Null);
        assert_eq!(body["data"]["period"], Value::Null);
    }

    #[tokio::test]
    async fn a_draft_with_current_reports_answers_200_and_queues_nothing() {
        let fixture = fixture();
        let (clip, _) = ready(&fixture);

        let (status, body) = get_json(&fixture, &format!("/api/v1/clips/{clip}/candidates")).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["job"], Value::Null);
        assert_eq!(body["data"]["start"]["around"], 1_020.0);
        assert_eq!(body["data"]["end"]["window"]["end"], 1_040.0);
        assert_eq!(job_count(&fixture), 0);
    }

    #[tokio::test]
    async fn an_edge_moved_far_from_where_it_was_examined_is_asked_for_again() {
        let fixture = fixture();
        let (clip, _) = ready(&fixture);
        lock(&fixture.state.database)
            .unwrap()
            .connection()
            .execute("UPDATE clips SET end_time = 1_100.0", [])
            .unwrap();

        let (status, body) = get_json(&fixture, &format!("/api/v1/clips/{clip}/candidates")).await;

        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(body["data"]["end"], Value::Null);
        assert_eq!(body["data"]["start"]["around"], 1_020.0);
    }

    #[tokio::test]
    async fn an_edge_moved_a_little_keeps_its_report() {
        let fixture = fixture();
        let (clip, _) = ready(&fixture);
        lock(&fixture.state.database)
            .unwrap()
            .connection()
            .execute("UPDATE clips SET end_time = 1_034.0", [])
            .unwrap();

        let (status, body) = get_json(&fixture, &format!("/api/v1/clips/{clip}/candidates")).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["end"]["around"], 1_020.0);
        assert_eq!(job_count(&fixture), 0);
    }

    #[tokio::test]
    async fn an_exported_clip_never_queues_a_job() {
        let fixture = fixture();
        let clip = insert_clip(&fixture, "exported", 1_010.0, 1_030.0);

        let (status, body) = get_json(&fixture, &format!("/api/v1/clips/{clip}/candidates")).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["job"], Value::Null);
        assert_eq!(body["data"]["start"], Value::Null);
        assert_eq!(job_count(&fixture), 0);
    }

    #[tokio::test]
    async fn candidates_of_an_unknown_clip_are_not_found() {
        let fixture = fixture();

        let (status, body) = get_json(&fixture, "/api/v1/clips/99/candidates").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn the_candidates_say_nothing_of_urls_or_paths() {
        let fixture = fixture();
        let queued = insert_clip(&fixture, "draft", 1_010.0, 1_030.0);
        let (current, _) = ready(&fixture);
        let work = fixture.state.library.clip_work(current);
        let work = work.parent().unwrap().to_string_lossy().into_owned();

        for clip in [queued, current] {
            let response = raw(&fixture, &format!("/api/v1/clips/{clip}/candidates"), None).await;
            let text = String::from_utf8(body_of(response).await).unwrap();
            assert!(!text.contains("http"), "{text}");
            assert!(!text.contains(&work), "{text}");
        }
    }

    #[tokio::test]
    async fn a_range_is_answered_with_that_part_of_the_preview() {
        let fixture = fixture();
        let (_, uri) = ready(&fixture);

        let response = raw(&fixture, &uri, Some("bytes=10-19")).await;

        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header_text(&response, header::CONTENT_RANGE).as_deref(),
            Some("bytes 10-19/1000")
        );
        assert_eq!(body_of(response).await, bytes()[10..20]);
    }

    #[tokio::test]
    async fn an_open_ended_range_runs_to_the_end() {
        let fixture = fixture();
        let (_, uri) = ready(&fixture);

        let response = raw(&fixture, &uri, Some("bytes=990-")).await;

        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header_text(&response, header::CONTENT_RANGE).as_deref(),
            Some("bytes 990-999/1000")
        );
        assert_eq!(body_of(response).await, bytes()[990..]);
    }

    #[tokio::test]
    async fn a_suffix_range_is_the_last_bytes() {
        let fixture = fixture();
        let (_, uri) = ready(&fixture);

        let response = raw(&fixture, &uri, Some("bytes=-5")).await;

        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header_text(&response, header::CONTENT_RANGE).as_deref(),
            Some("bytes 995-999/1000")
        );
        assert_eq!(body_of(response).await, bytes()[995..]);
    }

    #[tokio::test]
    async fn a_range_past_the_end_is_refused() {
        let fixture = fixture();
        let (_, uri) = ready(&fixture);

        let response = raw(&fixture, &uri, Some("bytes=2000-")).await;

        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    }

    #[tokio::test]
    async fn without_a_range_the_whole_preview_is_sent_with_its_headers() {
        let fixture = fixture();
        let (_, uri) = ready(&fixture);

        let response = raw(&fixture, &uri, None).await;

        assert_eq!(response.status(), StatusCode::OK);
        let text = |name| header_text(&response, name);
        assert_eq!(text(header::CONTENT_TYPE).as_deref(), Some("video/mp4"));
        assert_eq!(text(header::ACCEPT_RANGES).as_deref(), Some("bytes"));
        assert_eq!(text(header::CACHE_CONTROL).as_deref(), Some("no-cache"));
        let known = [
            header::CONTENT_TYPE,
            header::CONTENT_LENGTH,
            header::ACCEPT_RANGES,
            header::CACHE_CONTROL,
            header::LAST_MODIFIED,
            header::ETAG,
        ];
        let names: Vec<_> = response.headers().keys().cloned().collect();
        assert!(names.iter().all(|name| known.contains(name)), "{names:?}");
        assert_eq!(body_of(response).await, bytes());
    }

    #[tokio::test]
    async fn an_unknown_edge_is_a_bad_request() {
        let fixture = fixture();
        let (clip, _) = ready(&fixture);

        for query in ["?edge=middle", ""] {
            let uri = format!("/api/v1/clips/{clip}/preview{query}");
            let (status, body) = get_json(&fixture, &uri).await;

            assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
            assert_eq!(body["error"]["code"], "invalid_request");
        }
    }

    #[tokio::test]
    async fn a_missing_or_outdated_preview_is_not_found() {
        let fixture = fixture();
        let bare = insert_clip(&fixture, "draft", 1_010.0, 1_030.0);
        let (outdated, _) = ready(&fixture);
        lock(&fixture.state.database)
            .unwrap()
            .connection()
            .execute(
                "UPDATE clips SET start_time = 2_000.0 WHERE id = ?1",
                [outdated],
            )
            .unwrap();

        for clip in [bare, outdated, 99] {
            let uri = format!("/api/v1/clips/{clip}/preview?edge=start");
            let (status, body) = get_json(&fixture, &uri).await;

            assert_eq!(status, StatusCode::NOT_FOUND, "{clip}");
            assert_eq!(body["error"]["code"], "not_found");
        }
    }

    #[tokio::test]
    async fn the_sheet_is_served_as_a_jpeg() {
        let fixture = fixture();
        let (clip, _) = ready(&fixture);

        let response = raw(
            &fixture,
            &format!("/api/v1/clips/{clip}/sheet?edge=end"),
            None,
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_text(&response, header::CONTENT_TYPE).as_deref(),
            Some("image/jpeg")
        );
        assert_eq!(body_of(response).await, b"jpeg");
    }

    #[tokio::test]
    async fn a_sheet_with_a_bad_edge_is_a_bad_request() {
        let fixture = fixture();
        let (clip, _) = ready(&fixture);

        let (status, _) = get_json(&fixture, &format!("/api/v1/clips/{clip}/sheet?edge=x")).await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}

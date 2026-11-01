//! The HTTP API. Web UI and CLI call the same endpoints (FR-C1).
//!
//! Handlers lock the database on a blocking thread, as a long feature run can hold it. Errors
//! carry a stable code, and internals stay in the logs.

use crate::runner::now_ms;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use dalsegno_app::acquisition::Library;
use dalsegno_app::error::AcquireError;
use dalsegno_app::fetch::{SharedDatabase, lock};
use dalsegno_store::database::Database;
use dalsegno_store::jobs::{JobRow, JobState};
use dalsegno_store::status::{ChangeStatus, Listing, RegionStatus, StreamStatus};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::task::spawn_blocking;

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;

/// What the handlers share.
#[derive(Debug, Clone)]
pub struct ApiState {
    library: Library,
    database: SharedDatabase,
}

impl ApiState {
    /// Makes the state.
    pub fn new(library: Library) -> Self {
        let database = library.database();
        Self { library, database }
    }
}

#[derive(Debug, Serialize)]
struct Meta {
    total: u64,
    page: u32,
    limit: u32,
}

#[derive(Debug, Serialize)]
struct Envelope {
    success: bool,
    data: Value,
    error: Option<ErrorBody>,
    meta: Option<Meta>,
}

#[derive(Debug, Clone, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}

/// A failed request, as the caller sees it.
#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    body: ErrorBody,
}

impl ApiError {
    fn new(status: StatusCode, code: &'static str, message: &str) -> Self {
        Self {
            status,
            body: ErrorBody {
                code,
                message: message.to_string(),
            },
        }
    }

    fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "no such item")
    }

    fn internal(detail: &str) -> Self {
        tracing::error!(detail, "request failed");
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "the request failed, see the service log",
        )
    }
}

impl From<AcquireError> for ApiError {
    fn from(error: AcquireError) -> Self {
        match error {
            AcquireError::InvalidRequest(message) => {
                Self::new(StatusCode::CONFLICT, "invalid_request", message)
            }
            other => Self::internal(&other.to_string()),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let envelope = Envelope {
            success: false,
            data: Value::Null,
            error: Some(self.body),
            meta: None,
        };
        (self.status, Json(envelope)).into_response()
    }
}

fn ok(data: Value) -> Json<Envelope> {
    Json(Envelope {
        success: true,
        data,
        error: None,
        meta: None,
    })
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct Paging {
    page: Option<u32>,
    limit: Option<u32>,
}

impl Paging {
    fn limit(self) -> u32 {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }

    fn page(self) -> u32 {
        self.page.unwrap_or(1).max(1)
    }

    fn offset(self) -> u32 {
        (self.page() - 1).saturating_mul(self.limit())
    }

    fn envelope(self, total: u64, items: Vec<Value>) -> Json<Envelope> {
        Json(Envelope {
            success: true,
            data: Value::Array(items),
            error: None,
            meta: Some(Meta {
                total,
                page: self.page(),
                limit: self.limit(),
            }),
        })
    }
}

/// Runs a read or write on a blocking thread.
async fn with_database<T: Send + 'static>(
    database: &SharedDatabase,
    work: impl FnOnce(&Database) -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    let database = database.clone();
    spawn_blocking(move || {
        let guard = lock(&database).map_err(|error| ApiError::internal(&error.to_string()))?;
        work(&guard)
    })
    .await
    .map_err(|error| ApiError::internal(&error.to_string()))?
}

fn store_error(error: impl ToString) -> ApiError {
    ApiError::internal(&error.to_string())
}

/// Reads stored JSON, which is written by this program.
fn stored(text: Option<&str>) -> Value {
    text.and_then(|text| serde_json::from_str(text).ok())
        .unwrap_or(Value::Null)
}

fn stream_json(stream: &StreamStatus) -> Value {
    json!({
        "id": stream.id,
        "video_id": stream.video_id,
        "title": stream.title,
        "state": stream.state.as_str(),
        "state_reason": stream.state_reason,
        "period": stream.period,
        "follow_sq": stream.follow_sq,
        "last_fetch_at": stream.last_fetch_at,
    })
}

fn change_json(change: &ChangeStatus) -> Value {
    json!({
        "id": change.id,
        "program_id": change.program_id,
        "stream_id": change.stream_id,
        "state": change.state,
        "detected_at": change.detected_at,
        "dvr_deadline_at": change.dvr_deadline_at,
        "start_time": change.start_time,
        "checks": stored(change.checks.as_deref()),
        "video_boundaries": stored(change.boundaries.as_deref()),
    })
}

fn region_json(region: &RegionStatus) -> Value {
    json!({
        "class": region.class,
        "start": region.start,
        "end": region.end,
        "match_start": region.match_start,
    })
}

fn job_state(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Running => "running",
        JobState::Succeeded => "succeeded",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
    }
}

fn job_json(job: &JobRow) -> Value {
    json!({
        "id": job.id,
        "kind": job.job.kind,
        "target": stored(Some(&job.job.target)),
        "state": job_state(job.state),
        "progress": job.progress,
        "error": job.error,
        "created_at": job.created_at,
        "started_at": job.started_at,
        "finished_at": job.finished_at,
    })
}

async fn list_streams(
    State(state): State<ApiState>,
    Query(paging): Query<Paging>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        let total = database.count_rows(Listing::Streams).map_err(store_error)?;
        let rows = database
            .list_streams(paging.limit(), paging.offset())
            .map_err(store_error)?;
        Ok(paging.envelope(total, rows.iter().map(stream_json).collect()))
    })
    .await
}

async fn stream_detail(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        let stream = database.stream_status(id).map_err(store_error)?;
        Ok(ok(stream_json(&stream.ok_or_else(ApiError::not_found)?)))
    })
    .await
}

/// Runs a state change on a stream, and returns the stream.
async fn change_stream(
    state: ApiState,
    id: i64,
    change: impl FnOnce(&Library, &str) -> Result<(), AcquireError> + Send + 'static,
) -> Result<Json<Envelope>, ApiError> {
    let library = state.library.clone();
    let database = state.database.clone();
    spawn_blocking(move || {
        let video_id = {
            let guard = lock(&database).map_err(|error| ApiError::internal(&error.to_string()))?;
            let stream = guard.stream_status(id).map_err(store_error)?;
            stream.ok_or_else(ApiError::not_found)?.video_id
        };
        change(&library, &video_id)?;
        let guard = lock(&database).map_err(|error| ApiError::internal(&error.to_string()))?;
        let stream = guard.stream_status(id).map_err(store_error)?;
        Ok(ok(stream_json(&stream.ok_or_else(ApiError::not_found)?)))
    })
    .await
    .map_err(|error| ApiError::internal(&error.to_string()))?
}

async fn start_stream(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<Json<Envelope>, ApiError> {
    change_stream(state, id, |library, video| {
        library.start_stream(video, now_ms()).map(drop)
    })
    .await
}

async fn stop_stream(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<Json<Envelope>, ApiError> {
    change_stream(state, id, |library, video| {
        library.stop_stream(video).map(drop)
    })
    .await
}

async fn list_changes(
    State(state): State<ApiState>,
    Query(paging): Query<Paging>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        let total = database.count_rows(Listing::Changes).map_err(store_error)?;
        let rows = database
            .list_changes(paging.limit(), paging.offset())
            .map_err(store_error)?;
        Ok(paging.envelope(total, rows.iter().map(change_json).collect()))
    })
    .await
}

async fn change_detail(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        let change = database.change_status(id).map_err(store_error)?;
        let change = change.ok_or_else(ApiError::not_found)?;
        let regions = database.change_regions(id).map_err(store_error)?;
        let mut body = change_json(&change);
        body["regions"] = regions.iter().map(region_json).collect();
        Ok(ok(body))
    })
    .await
}

async fn list_jobs(
    State(state): State<ApiState>,
    Query(paging): Query<Paging>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        let total = database.count_rows(Listing::Jobs).map_err(store_error)?;
        let rows = database
            .list_jobs(paging.limit(), paging.offset())
            .map_err(store_error)?;
        Ok(paging.envelope(total, rows.iter().map(job_json).collect()))
    })
    .await
}

async fn cancel_job(
    State(state): State<ApiState>,
    Path(id): Path<i64>,
) -> Result<Json<Envelope>, ApiError> {
    with_database(&state.database, move |database| {
        database
            .job(id)
            .map_err(store_error)?
            .ok_or_else(ApiError::not_found)?;
        let cancelled = database.cancel_job(id, now_ms()).map_err(store_error)?;
        if !cancelled {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "invalid_request",
                "the job has finished",
            ));
        }
        Ok(ok(json!({ "id": id, "state": "cancelled" })))
    })
    .await
}

/// Returns whether a request comes from a page of another site. Browsers set `Origin` on such
/// requests, and the CLI sends none.
fn is_foreign(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return false;
    };
    let origin_host = origin
        .to_str()
        .ok()
        .and_then(|origin| origin.split_once("://"))
        .map(|(_, host)| host);
    let host = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok());
    origin_host.is_none() || origin_host != host
}

/// Refuses state changes from other sites, whatever a proxy in front lets through (D-20).
async fn reject_foreign_origin(request: Request, next: Next) -> Response {
    let changes_state = !matches!(*request.method(), Method::GET | Method::HEAD);
    if changes_state && is_foreign(request.headers()) {
        return ApiError::new(StatusCode::FORBIDDEN, "forbidden", "foreign origin").into_response();
    }
    next.run(request).await
}

async fn unknown_route() -> ApiError {
    ApiError::not_found()
}

/// Builds the API.
pub fn router(state: ApiState) -> Router {
    let api = Router::new()
        .route("/streams", get(list_streams))
        .route("/streams/{id}", get(stream_detail))
        .route("/streams/{id}/start", post(start_stream))
        .route("/streams/{id}/stop", post(stop_stream))
        .route("/changes", get(list_changes))
        .route("/changes/{id}", get(change_detail))
        .route("/jobs", get(list_jobs))
        .route("/jobs/{id}/cancel", post(cancel_job));
    Router::new()
        .nest("/api/v1", api)
        .fallback(unknown_route)
        .layer(middleware::from_fn(reject_foreign_origin))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use dalsegno_store::jobs::{JobClass, NewJob};
    use dalsegno_store::streams::{NewStream, StreamState};
    use tempfile::{TempDir, tempdir};
    use tower::ServiceExt;

    const BODY_LIMIT: usize = 1 << 20;

    struct Fixture {
        _root: TempDir,
        state: ApiState,
        stream: i64,
    }

    fn fixture() -> Fixture {
        let root = tempdir().unwrap();
        let library = Library::open(root.path()).unwrap();
        let state = ApiState::new(library);
        let stream = lock(&state.database)
            .unwrap()
            .insert_stream(&NewStream {
                video_id: "abcDEF123_-".to_string(),
                title: "Example TV".to_string(),
                channel_id: None,
                channel_name: None,
                thumbnail_url: None,
                video_itag: None,
                audio_itag: 140,
                fragment_len: 5.0,
                registered_at: 0,
            })
            .unwrap();
        Fixture {
            _root: root,
            state,
            stream,
        }
    }

    async fn send(fixture: &Fixture, request: Request<Body>) -> (StatusCode, Value) {
        let response = router(fixture.state.clone())
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), BODY_LIMIT).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn get_json(fixture: &Fixture, uri: &str) -> (StatusCode, Value) {
        let request = Request::get(uri).body(Body::empty()).unwrap();
        send(fixture, request).await
    }

    async fn post_json(fixture: &Fixture, uri: &str) -> (StatusCode, Value) {
        let request = Request::post(uri).body(Body::empty()).unwrap();
        send(fixture, request).await
    }

    fn set_state(fixture: &Fixture, state: StreamState) {
        lock(&fixture.state.database)
            .unwrap()
            .set_stream_state(fixture.stream, state, None)
            .unwrap();
    }

    #[tokio::test]
    async fn streams_are_listed_in_the_envelope() {
        let fixture = fixture();

        let (status, body) = get_json(&fixture, "/api/v1/streams").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["success"], true);
        assert_eq!(body["error"], Value::Null);
        assert_eq!(body["data"][0]["state"], "registered");
        assert_eq!(body["meta"]["total"], 1);
        assert_eq!(body["meta"]["limit"], DEFAULT_LIMIT);
    }

    #[tokio::test]
    async fn a_page_past_the_end_is_empty() {
        let fixture = fixture();

        let (_, body) = get_json(&fixture, "/api/v1/streams?page=9&limit=1").await;

        assert_eq!(body["data"], json!([]));
        assert_eq!(body["meta"]["page"], 9);
    }

    #[tokio::test]
    async fn a_limit_is_kept_in_range() {
        let fixture = fixture();

        let (_, body) = get_json(&fixture, "/api/v1/streams?limit=999999").await;

        assert_eq!(body["meta"]["limit"], MAX_LIMIT);
    }

    #[tokio::test]
    async fn an_unknown_stream_has_a_stable_code() {
        let fixture = fixture();

        let (status, body) = get_json(&fixture, "/api/v1/streams/99").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["success"], false);
        assert_eq!(body["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn an_unknown_route_answers_in_the_envelope() {
        let fixture = fixture();

        let (status, body) = get_json(&fixture, "/api/v1/nothing").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn starting_and_stopping_change_the_state() {
        let fixture = fixture();
        let uri = format!("/api/v1/streams/{}", fixture.stream);

        let (_, started) = post_json(&fixture, &format!("{uri}/start")).await;
        let (_, stopped) = post_json(&fixture, &format!("{uri}/stop")).await;

        assert_eq!(started["data"]["state"], "backfilling");
        assert_eq!(stopped["data"]["state"], "paused");
    }

    #[tokio::test]
    async fn a_finished_stream_cannot_start() {
        let fixture = fixture();
        set_state(&fixture, StreamState::Ended);
        let uri = format!("/api/v1/streams/{}/start", fixture.stream);

        let (status, body) = post_json(&fixture, &uri).await;

        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "invalid_request");
    }

    #[tokio::test]
    async fn starting_an_unknown_stream_is_not_found() {
        let fixture = fixture();

        let (status, _) = post_json(&fixture, "/api/v1/streams/99/start").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_foreign_origin_cannot_change_state() {
        let fixture = fixture();
        let request = Request::post(format!("/api/v1/streams/{}/start", fixture.stream))
            .header(header::ORIGIN, "https://evil.example")
            .header(header::HOST, "127.0.0.1:8737")
            .body(Body::empty())
            .unwrap();

        let (status, body) = send(&fixture, request).await;

        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error"]["code"], "forbidden");
        let database = lock(&fixture.state.database).unwrap();
        let state = database.stream_state(fixture.stream).unwrap().unwrap().0;
        assert_eq!(state, StreamState::Registered);
    }

    #[tokio::test]
    async fn the_same_origin_can_change_state() {
        let fixture = fixture();
        let request = Request::post(format!("/api/v1/streams/{}/start", fixture.stream))
            .header(header::ORIGIN, "http://127.0.0.1:8737")
            .header(header::HOST, "127.0.0.1:8737")
            .body(Body::empty())
            .unwrap();

        assert_eq!(send(&fixture, request).await.0, StatusCode::OK);
    }

    #[tokio::test]
    async fn a_foreign_origin_can_still_read() {
        let fixture = fixture();
        let request = Request::get("/api/v1/streams")
            .header(header::ORIGIN, "https://evil.example")
            .body(Body::empty())
            .unwrap();

        assert_eq!(send(&fixture, request).await.0, StatusCode::OK);
    }

    #[test]
    fn a_broken_origin_is_foreign() {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, "null".parse().unwrap());
        headers.insert(header::HOST, "null".parse().unwrap());

        assert!(is_foreign(&headers));
    }

    #[tokio::test]
    async fn an_unknown_change_is_not_found() {
        let fixture = fixture();

        assert_eq!(
            get_json(&fixture, "/api/v1/changes/1").await.0,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn jobs_can_be_listed_and_cancelled_once() {
        let fixture = fixture();
        let job = NewJob {
            kind: "sweep".to_string(),
            class: JobClass::Cpu,
            target: "{\"a\":1}".to_string(),
        };
        let id = lock(&fixture.state.database)
            .unwrap()
            .enqueue_job(&job, 0)
            .unwrap();

        let (_, listed) = get_json(&fixture, "/api/v1/jobs").await;
        let uri = format!("/api/v1/jobs/{id}/cancel");
        let (first, _) = post_json(&fixture, &uri).await;
        let (second, _) = post_json(&fixture, &uri).await;

        assert_eq!(listed["data"][0]["target"]["a"], 1);
        assert_eq!(first, StatusCode::OK);
        assert_eq!(second, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn cancelling_an_unknown_job_is_not_found() {
        let fixture = fixture();

        assert_eq!(
            post_json(&fixture, "/api/v1/jobs/9/cancel").await.0,
            StatusCode::NOT_FOUND
        );
    }
}

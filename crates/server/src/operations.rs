//! Settings, diagnostics and jobs asked for by hand (FR-U6, FR-C1, FR-C2, D-24).

use crate::api::{ApiError, ApiState, Envelope, job_json, ok, store_error, with_database};
use crate::runner::now_ms;
use crate::scheduler::SWEEP;
use axum::Json;
use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use dalsegno_app::lifecycle::{FetchTarget, StreamTarget};
use dalsegno_app::settings::SettingsError;
use dalsegno_store::database::Database;
use dalsegno_store::jobs::{JobClass, NewJob};
use dalsegno_store::streams::StreamRow;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use tokio::task::spawn_blocking;

/// The DVR keeps five days, so a backfill cannot reach further.
const MAX_BACKFILL_HOURS: f64 = 120.0;
/// About five days of fragments, which is all the DVR holds.
const MAX_FETCH_FRAGMENTS: u64 = 100_000;

fn invalid(message: &str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
}

/// Reads a JSON body into a validated type, answering in the envelope when it does not read.
fn parse_body<T: DeserializeOwned>(body: &Bytes) -> Result<T, ApiError> {
    serde_json::from_slice(body)
        .map_err(|error| invalid(&format!("the body is not valid: {error}")))
}

fn settings_json(state: &ApiState) -> Value {
    json!({
        "operational": state.services.settings.view(),
        "bootstrap": state.services.bootstrap,
    })
}

fn settings_error(error: SettingsError) -> ApiError {
    let message = error.to_string();
    let code = match error {
        SettingsError::Bootstrap(_) => "bootstrap_setting",
        SettingsError::Unknown(_) => "unknown_setting",
        SettingsError::Invalid { .. } => "invalid_setting",
        SettingsError::Store(detail) => return ApiError::internal(&detail),
    };
    ApiError::new(StatusCode::BAD_REQUEST, code, &message)
}

/// The operational settings in force and the bootstrap values, secrets shown only as set.
pub(crate) async fn get_settings(State(state): State<ApiState>) -> Json<Envelope> {
    ok(settings_json(&state))
}

/// Changes operational settings, which apply at once, and answers as `GET` does.
pub(crate) async fn put_settings(
    State(state): State<ApiState>,
    body: Bytes,
) -> Result<Json<Envelope>, ApiError> {
    let change: Map<String, Value> = parse_body(&body)?;
    let settings = state.services.settings.clone();
    spawn_blocking(move || settings.update(&change))
        .await
        .map_err(|error| ApiError::internal(&error.to_string()))?
        .map_err(settings_error)?;
    Ok(ok(settings_json(&state)))
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub(crate) struct DoctorQuery {
    stream: Option<i64>,
}

async fn video_id(state: &ApiState, stream: i64) -> Result<String, ApiError> {
    with_database(&state.database, move |database| {
        Ok(stream_row(database, stream)?.stream.video_id)
    })
    .await
}

/// Runs the diagnostics in the service, so they show its own family state, and with a
/// stream also resolves its URL and probes its formats.
pub(crate) async fn doctor(
    State(state): State<ApiState>,
    Query(query): Query<DoctorQuery>,
) -> Result<Json<Envelope>, ApiError> {
    let video = match query.stream {
        Some(stream) => Some(video_id(&state, stream).await?),
        None => None,
    };
    let acquisition = &state.services.acquisition;
    let report = acquisition.doctor(video.as_deref()).await?;
    let probe = match video.as_deref() {
        Some(video) => Some(match acquisition.probe(video).await {
            Ok(probe) => json!(probe),
            Err(error) => {
                // The reason can quote the origin, so it stays in the log.
                tracing::warn!(error = %error, "the probe failed");
                json!({ "error": "the probe failed, see the service log" })
            }
        }),
        None => None,
    };
    Ok(ok(json!({ "doctor": report, "probe": probe })))
}

/// A job asked for by hand.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum JobRequest {
    Backfill {
        stream: i64,
        hours: Option<f64>,
    },
    Features {
        stream: i64,
    },
    Analysis {
        stream: i64,
    },
    Sweep,
    Fetch {
        stream: i64,
        itag: u32,
        first: u64,
        last: u64,
    },
}

fn stream_row(database: &Database, stream: i64) -> Result<StreamRow, ApiError> {
    database
        .stream(stream)
        .map_err(store_error)?
        .ok_or_else(ApiError::not_found)
}

fn new_job(database: &Database, request: &JobRequest) -> Result<NewJob, ApiError> {
    let job = match *request {
        JobRequest::Sweep => NewJob {
            kind: SWEEP.to_string(),
            class: JobClass::Cpu,
            target: "{}".to_string(),
        },
        JobRequest::Backfill { stream, hours } => {
            let row = stream_row(database, stream)?;
            match hours {
                None => StreamTarget::backfill(&row)?,
                Some(hours) if hours > 0.0 && hours <= MAX_BACKFILL_HOURS => {
                    StreamTarget::backfill_hours(&row, hours)?
                }
                Some(_) => return Err(invalid("hours must be more than 0 and at most 120")),
            }
        }
        JobRequest::Features { stream } => StreamTarget::features(&stream_row(database, stream)?)?,
        JobRequest::Analysis { stream } => StreamTarget::analysis(&stream_row(database, stream)?)?,
        JobRequest::Fetch {
            stream,
            itag,
            first,
            last,
        } => {
            if first > last || last - first >= MAX_FETCH_FRAGMENTS {
                return Err(invalid(
                    "the range must run forwards and hold at most 100000 fragments",
                ));
            }
            let row = stream_row(database, stream)?;
            FetchTarget {
                stream: row.stream.video_id,
                itag,
                first,
                last,
            }
            .job()?
        }
    };
    Ok(job)
}

/// Queues a job and answers with it, as it is queued (202).
pub(crate) async fn create_job(
    State(state): State<ApiState>,
    body: Bytes,
) -> Result<(StatusCode, Json<Envelope>), ApiError> {
    let request: JobRequest = parse_body(&body)?;
    let job = with_database(&state.database, move |database| {
        let job = new_job(database, &request)?;
        let id = database.enqueue_job(&job, now_ms()).map_err(store_error)?;
        let row = database.job(id).map_err(store_error)?;
        Ok(job_json(&row.ok_or_else(ApiError::not_found)?))
    })
    .await?;
    Ok((StatusCode::ACCEPTED, ok(job)))
}

#[cfg(test)]
mod tests {
    use crate::api::tests::{Fixture, fixture, get_json, send};
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use serde_json::{Value, json};

    async fn with_body(
        fixture: &Fixture,
        method: &str,
        uri: &str,
        body: Value,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        send(fixture, request).await
    }

    #[tokio::test]
    async fn settings_show_what_is_in_force_and_the_bootstrap_values() {
        let fixture = fixture();

        let (status, body) = get_json(&fixture, "/api/v1/settings").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body["data"]["operational"]["daily_summary"]["value"],
            "09:00"
        );
        assert_eq!(
            body["data"]["operational"]["concurrency"]["overridden"],
            false
        );
        assert_eq!(body["data"]["bootstrap"]["webhook"], true);
    }

    #[tokio::test]
    async fn a_change_of_settings_applies_at_once() {
        let fixture = fixture();
        let mut updates = fixture.state.services.settings.subscribe();

        let (status, body) = with_body(
            &fixture,
            "PUT",
            "/api/v1/settings",
            json!({ "concurrency": 2, "daily_summary": "off" }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["operational"]["concurrency"]["value"], 2);
        assert_eq!(body["data"]["operational"]["daily_summary"]["value"], "off");
        assert!(updates.has_changed().unwrap());
        assert_eq!(updates.borrow_and_update().limits.concurrency, 2);
    }

    #[tokio::test]
    async fn a_bootstrap_key_or_a_body_that_does_not_read_is_refused() {
        let fixture = fixture();

        let (bootstrap, bootstrap_body) = with_body(
            &fixture,
            "PUT",
            "/api/v1/settings",
            json!({ "webhook": "x" }),
        )
        .await;
        let (invalid, invalid_body) =
            with_body(&fixture, "PUT", "/api/v1/settings", json!([1, 2])).await;
        let (range, range_body) = with_body(
            &fixture,
            "PUT",
            "/api/v1/settings",
            json!({ "concurrency": 0 }),
        )
        .await;

        assert_eq!(bootstrap, StatusCode::BAD_REQUEST);
        assert_eq!(bootstrap_body["error"]["code"], "bootstrap_setting");
        assert_eq!(invalid, StatusCode::BAD_REQUEST);
        assert_eq!(invalid_body["error"]["code"], "invalid_request");
        assert_eq!(range, StatusCode::BAD_REQUEST);
        assert_eq!(range_body["error"]["code"], "invalid_setting");
    }

    #[tokio::test]
    async fn a_job_asked_for_is_queued_and_answered_with_202() {
        let fixture = fixture();
        let stream = fixture.stream;

        let (backfill, backfill_body) = with_body(
            &fixture,
            "POST",
            "/api/v1/jobs",
            json!({ "kind": "backfill", "stream": stream, "hours": 6 }),
        )
        .await;
        let (sweep, sweep_body) =
            with_body(&fixture, "POST", "/api/v1/jobs", json!({ "kind": "sweep" })).await;
        let (fetch, fetch_body) = with_body(
            &fixture,
            "POST",
            "/api/v1/jobs",
            json!({ "kind": "fetch", "stream": stream, "itag": 140, "first": 10, "last": 20 }),
        )
        .await;

        assert_eq!(backfill, StatusCode::ACCEPTED);
        assert_eq!(backfill_body["data"]["kind"], "backfill");
        assert_eq!(backfill_body["data"]["state"], "queued");
        assert_eq!(backfill_body["data"]["target"]["hours"], 6.0);
        assert_eq!(sweep, StatusCode::ACCEPTED);
        assert_eq!(sweep_body["data"]["kind"], "sweep");
        assert_eq!(fetch, StatusCode::ACCEPTED);
        assert_eq!(fetch_body["data"]["target"]["itag"], 140);
    }

    #[tokio::test]
    async fn a_job_that_cannot_run_is_refused() {
        let fixture = fixture();
        let stream = fixture.stream;
        let cases = [
            (
                json!({ "kind": "export", "stream": stream }),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({ "kind": "features", "stream": 999 }),
                StatusCode::NOT_FOUND,
            ),
            (
                json!({ "kind": "backfill", "stream": stream, "hours": 200 }),
                StatusCode::BAD_REQUEST,
            ),
            (
                json!({ "kind": "fetch", "stream": stream, "itag": 140, "first": 9, "last": 3 }),
                StatusCode::BAD_REQUEST,
            ),
        ];

        for (request, expected) in cases {
            let (status, body) = with_body(&fixture, "POST", "/api/v1/jobs", request.clone()).await;
            assert_eq!(status, expected, "{request}");
            assert_eq!(body["success"], false);
        }
        let (_, jobs) = get_json(&fixture, "/api/v1/jobs").await;
        assert_eq!(jobs["meta"]["total"], 0);
    }

    #[tokio::test]
    async fn doctor_runs_in_the_service_and_needs_a_known_stream() {
        let fixture = fixture();

        let (status, body) = get_json(&fixture, "/api/v1/doctor").await;
        let (unknown, _) = get_json(&fixture, "/api/v1/doctor?stream=999").await;

        assert_eq!(status, StatusCode::OK);
        assert!(body["data"]["doctor"]["tools"].is_array());
        assert_eq!(body["data"]["doctor"]["policy"], "v4-only");
        assert!(body["data"]["probe"].is_null());
        assert_eq!(unknown, StatusCode::NOT_FOUND);
    }
}

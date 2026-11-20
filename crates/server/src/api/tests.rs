use super::*;
use axum::body::{Body, to_bytes};
use axum::http::Request;
use chrono::NaiveTime;
use dalsegno_app::acquisition::{AcquisitionOptions, IpPolicy};
use dalsegno_app::fetch::FetchLimits;
use dalsegno_app::settings::Operational;
use dalsegno_store::jobs::{JobClass, NewJob};
use dalsegno_store::streams::{NewStream, StreamState};
use futures_util::StreamExt;
use tempfile::{TempDir, tempdir};
use tower::ServiceExt;

const BODY_LIMIT: usize = 1 << 20;

pub(crate) struct Fixture {
    _root: TempDir,
    pub(crate) state: ApiState,
    pub(crate) stream: i64,
}

/// Settings that start from the defaults, with the daily summary at 09:00.
fn services(library: &Library) -> Services {
    let base = Operational {
        limits: FetchLimits::default(),
        daily_summary: NaiveTime::from_hms_opt(9, 0, 0),
    };
    let (settings, _) = LiveSettings::load(base, library.database()).unwrap();
    let acquisition = Acquisition::new(AcquisitionOptions {
        policy: IpPolicy::V4Only,
        cookies: None,
        limits: FetchLimits::default(),
    })
    .unwrap();
    Services {
        acquisition,
        settings,
        bootstrap: json!({ "data": "/data", "webhook": true }),
    }
}

pub(crate) fn fixture() -> Fixture {
    let root = tempdir().unwrap();
    let library = Library::open(root.path()).unwrap();
    let services = services(&library);
    let state = ApiState::new(library, Some("https://dalsegno.example/"), services);
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

pub(crate) async fn send(fixture: &Fixture, request: Request<Body>) -> (StatusCode, Value) {
    let response = router(fixture.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), BODY_LIMIT).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

pub(crate) async fn get_json(fixture: &Fixture, uri: &str) -> (StatusCode, Value) {
    let request = Request::get(uri).body(Body::empty()).unwrap();
    send(fixture, request).await
}

pub(crate) async fn send_body(
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
    assert_eq!(body["data"][0]["open_fragments"], 0);
    assert_eq!(body["data"][0]["lost_fragments"], 0);
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
async fn a_page_outside_the_api_is_left_to_the_web_ui() {
    let fixture = fixture();
    let request = Request::get("/changes/7").body(Body::empty()).unwrap();

    let response = router(fixture.state.clone())
        .oneshot(request)
        .await
        .unwrap();

    // The build may or may not be there, but either way the answer is not the API's.
    let mime = response.headers().get(header::CONTENT_TYPE).unwrap();
    assert!(!mime.to_str().unwrap().starts_with("application/json"));
}

#[tokio::test]
async fn the_event_stream_carries_what_is_published() {
    let fixture = fixture();
    let request = Request::get("/api/v1/events").body(Body::empty()).unwrap();

    let response = router(fixture.state.clone())
        .oneshot(request)
        .await
        .unwrap();
    fixture.state.events().publish(
        crate::events::Topic::Stream,
        json!({ "id": fixture.stream }),
    );
    let headers = response.headers().clone();
    let mut body = response.into_body().into_data_stream();
    let opening = body.next().await.unwrap().unwrap();
    let chunk = body.next().await.unwrap().unwrap();

    assert_eq!(headers[header::CONTENT_TYPE], "text/event-stream");
    assert_eq!(headers["x-accel-buffering"], "no");
    let opening = String::from_utf8(opening.to_vec()).unwrap();
    assert!(opening.starts_with("event: resync\n"));
    assert!(opening.contains(env!("CARGO_PKG_VERSION")));
    let text = String::from_utf8(chunk.to_vec()).unwrap();
    assert!(text.contains("event: stream.state\n"));
    assert!(text.contains(&format!("{{\"id\":{}}}", fixture.stream)));
}

#[tokio::test]
async fn a_write_outside_the_api_is_not_found() {
    let fixture = fixture();
    let request = Request::post("/changes/7").body(Body::empty()).unwrap();

    let (status, body) = send(&fixture, request).await;

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

#[tokio::test]
async fn a_rebound_host_cannot_read_or_change_state() {
    let fixture = fixture();
    let read = Request::get("/api/v1/streams")
        .header(header::HOST, "rebind.example:8737")
        .body(Body::empty())
        .unwrap();
    let change = Request::post(format!("/api/v1/streams/{}/start", fixture.stream))
        .header(header::ORIGIN, "http://rebind.example:8737")
        .header(header::HOST, "rebind.example:8737")
        .body(Body::empty())
        .unwrap();

    assert_eq!(send(&fixture, read).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&fixture, change).await.0, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_request_without_a_host_passes() {
    let fixture = fixture();
    let request = Request::get("/api/v1/streams").body(Body::empty()).unwrap();

    assert_eq!(send(&fixture, request).await.0, StatusCode::OK);
}

#[tokio::test]
async fn the_public_host_can_change_state() {
    let fixture = fixture();
    let request = Request::post(format!("/api/v1/streams/{}/start", fixture.stream))
        .header(header::ORIGIN, "https://dalsegno.example")
        .header(header::HOST, "dalsegno.example")
        .body(Body::empty())
        .unwrap();

    assert_eq!(send(&fixture, request).await.0, StatusCode::OK);
}

#[test]
fn loopback_addresses_and_the_public_host_are_known() {
    let public = Some("dalsegno.example");

    for host in [
        "localhost:8737",
        "127.0.0.1:8737",
        "[::1]:8737",
        "172.18.0.5",
        "Dalsegno.Example",
    ] {
        assert!(is_known_host(host, public), "{host}");
    }
    for host in [
        "rebind.example",
        "dalsegno.example.evil",
        "not a host",
        "rebind.example@localhost",
    ] {
        assert!(!is_known_host(host, public), "{host}");
    }
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

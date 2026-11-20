//! Commands that go through the running service (FR-C1).

use crate::text::{doctor_text, probe_text};
use anyhow::anyhow;
use dalsegno_app::acquisition::video_id_of;
use dalsegno_app::doctor::DoctorReport;
use dalsegno_app::probe::ProbeReport;
use dalsegno_app::remote::ApiClient;
use serde_json::{Map, Value, json};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// As many rows as one page of the API holds.
const PAGE: &str = "?limit=200";
const MINUTE_MS: i64 = 60_000;
const MINUTES_PER_HOUR: i64 = 60;
/// How long `doctor` waits, since a probe fetches real fragments over each family.
const DOCTOR_WAIT: Duration = Duration::from_secs(5 * 60);

fn text<'a>(row: &'a Value, key: &str) -> &'a str {
    row[key].as_str().unwrap_or("-")
}

fn rows(data: &Value) -> &[Value] {
    data.as_array().map_or(&[], Vec::as_slice)
}

fn stream_line(row: &Value) -> String {
    let period = row["period"]
        .as_f64()
        .map_or("no period".to_string(), |period| format!("{period:.1} s"));
    let reason = row["state_reason"]
        .as_str()
        .map_or(String::new(), |reason| format!(" ({reason})"));
    let count = |key: &str| row[key].as_u64().map_or("-".to_string(), |n| n.to_string());
    format!(
        "{} {} {}{reason}, {}, next sq {}, {} to fill, {} lost: {}\n",
        row["id"],
        text(row, "video_id"),
        text(row, "state"),
        period,
        count("follow_sq"),
        count("open_fragments"),
        count("lost_fragments"),
        text(row, "title"),
    )
}

/// What is left of a job that has done `fraction` since `started_at`, at its pace so far.
fn left(fraction: f64, started_at: i64, now: i64) -> Option<String> {
    let elapsed = now - started_at;
    if !(fraction > 0.0 && fraction < 1.0) || elapsed < 0 {
        return None;
    }
    let minutes = (elapsed as f64 * (1.0 - fraction) / fraction / MINUTE_MS as f64).ceil();
    let minutes = (minutes as i64).max(1);
    let wait = match (minutes / MINUTES_PER_HOUR, minutes % MINUTES_PER_HOUR) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    };
    Some(format!("about {wait} left"))
}

/// How far a running job has come, once it has started to move.
fn job_progress(row: &Value, now: i64) -> String {
    let fraction = row["progress"].as_f64().unwrap_or(0.0);
    if row["state"] != "running" || !(fraction > 0.0 && fraction < 1.0) {
        return String::new();
    }
    let percent = (fraction * 100.0).floor();
    match row["started_at"]
        .as_i64()
        .and_then(|started| left(fraction, started, now))
    {
        Some(wait) => format!(" {percent}%, {wait}"),
        None => format!(" {percent}%"),
    }
}

fn job_line(row: &Value, now: i64) -> String {
    let error = row["error"]
        .as_str()
        .map_or(String::new(), |error| format!(": {error}"));
    format!(
        "{} {} {}{}{error}\n",
        row["id"],
        text(row, "state"),
        text(row, "kind"),
        job_progress(row, now),
    )
}

fn change_line(row: &Value) -> String {
    format!(
        "{} {} stream {}, from {:.1}\n",
        row["id"],
        text(row, "state"),
        row["stream_id"],
        row["start_time"].as_f64().unwrap_or(0.0),
    )
}

fn print_rows(json: bool, data: &Value, line: impl Fn(&Value) -> String) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(data)?);
    } else {
        print!("{}", rows(data).iter().map(line).collect::<String>());
    }
    Ok(())
}

/// Shows the streams.
pub(crate) async fn status(client: &ApiClient, json: bool) -> anyhow::Result<()> {
    let data = client.get(&format!("/streams{PAGE}")).await?;
    print_rows(json, &data, stream_line)
}

/// Shows the newest jobs.
pub(crate) async fn jobs(client: &ApiClient, json: bool) -> anyhow::Result<()> {
    let data = client.get(&format!("/jobs{PAGE}")).await?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as i64);
    print_rows(json, &data, |row| job_line(row, now))
}

/// Shows the newest changes.
pub(crate) async fn changes(client: &ApiClient, json: bool) -> anyhow::Result<()> {
    let data = client.get(&format!("/changes{PAGE}")).await?;
    print_rows(json, &data, change_line)
}

fn row_id(data: &Value, video_id: &str) -> Option<i64> {
    rows(data)
        .iter()
        .find(|row| row["video_id"] == video_id)
        .and_then(|row| row["id"].as_i64())
}

/// Finds the numeric ID of a stream from its video ID or URL.
async fn stream_id(client: &ApiClient, stream: &str) -> anyhow::Result<i64> {
    let video_id = video_id_of(stream)?;
    let streams = client.get(&format!("/streams{PAGE}")).await?;
    row_id(&streams, &video_id).ok_or_else(|| anyhow!("stream {video_id} is not registered"))
}

fn print_json(data: &Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(data)?);
    Ok(())
}

/// Calls `/streams/{id}/{action}` for a video ID or URL, and shows the stream.
pub(crate) async fn stream_action(
    client: &ApiClient,
    stream: &str,
    action: &str,
    json: bool,
) -> anyhow::Result<()> {
    let id = stream_id(client, stream).await?;
    let data = client.post(&format!("/streams/{id}/{action}")).await?;
    if json {
        print_json(&data)
    } else {
        print!("{}", stream_line(&data));
        Ok(())
    }
}

/// The doctor report, and what the probe found or why it failed.
fn doctor_output(data: &Value) -> anyhow::Result<String> {
    let doctor: DoctorReport = serde_json::from_value(data["doctor"].clone())?;
    let probe = &data["probe"];
    let probe_part = if let Some(message) = probe["error"].as_str() {
        format!("probe: {message}\n")
    } else if probe.is_object() {
        probe_text(&serde_json::from_value::<ProbeReport>(probe.clone())?)
    } else {
        String::new()
    };
    Ok(doctor_text(&doctor) + &probe_part)
}

/// Shows what the service finds on its host, and with a stream, what it finds of the stream.
pub(crate) async fn doctor(
    client: &ApiClient,
    stream: Option<&str>,
    json: bool,
) -> anyhow::Result<()> {
    let path = match stream {
        Some(stream) => format!("/doctor?stream={}", stream_id(client, stream).await?),
        None => "/doctor".to_string(),
    };
    let data = client.get_waiting(&path, DOCTOR_WAIT).await?;
    if json {
        print_json(&data)
    } else {
        print!("{}", doctor_output(&data)?);
        Ok(())
    }
}

/// Which job to queue, in the words of the CLI.
pub(crate) enum JobRequest<'a> {
    Backfill {
        stream: &'a str,
        hours: Option<f64>,
    },
    Features {
        stream: &'a str,
    },
    Sweep,
    Fetch {
        stream: &'a str,
        itag: u32,
        first: u64,
        last: u64,
    },
}

/// The body of `POST /jobs`, once the stream has its numeric ID.
fn job_body(request: &JobRequest, stream_id: Option<i64>) -> Value {
    match request {
        JobRequest::Backfill {
            hours: Some(hours), ..
        } => json!({ "kind": "backfill", "stream": stream_id, "hours": hours }),
        JobRequest::Backfill { hours: None, .. } => {
            json!({ "kind": "backfill", "stream": stream_id })
        }
        JobRequest::Features { .. } => json!({ "kind": "features", "stream": stream_id }),
        JobRequest::Sweep => json!({ "kind": "sweep" }),
        JobRequest::Fetch {
            itag, first, last, ..
        } => json!({
            "kind": "fetch",
            "stream": stream_id,
            "itag": itag,
            "first": first,
            "last": last,
        }),
    }
}

impl JobRequest<'_> {
    fn stream(&self) -> Option<&str> {
        match self {
            Self::Backfill { stream, .. }
            | Self::Features { stream }
            | Self::Fetch { stream, .. } => Some(stream),
            Self::Sweep => None,
        }
    }
}

/// Queues a job, and shows it the way `jobs` does.
pub(crate) async fn queue(
    client: &ApiClient,
    request: &JobRequest<'_>,
    json: bool,
) -> anyhow::Result<()> {
    let id = match request.stream() {
        Some(stream) => Some(stream_id(client, stream).await?),
        None => None,
    };
    let data = client.post_json("/jobs", &job_body(request, id)).await?;
    if json {
        print_json(&data)
    } else {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_millis() as i64);
        print!("{}", job_line(&data, now));
        Ok(())
    }
}

/// Reads `KEY=VALUE`, where a value that is JSON stays what it is and any other is text.
fn parse_setting(pair: &str) -> anyhow::Result<(String, Value)> {
    let (key, value) = pair
        .split_once('=')
        .filter(|(key, _)| !key.is_empty())
        .ok_or_else(|| anyhow!("expected KEY=VALUE, got {pair:?}"))?;
    let value = serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_string()));
    Ok((key.to_string(), value))
}

/// The body of `PUT /settings`: each pair sets a key and each reset drops its override.
fn settings_body(pairs: &[String], resets: &[String]) -> anyhow::Result<Value> {
    let mut body = Map::new();
    for pair in pairs {
        let (key, value) = parse_setting(pair)?;
        body.insert(key, value);
    }
    for key in resets {
        body.insert(key.clone(), Value::Null);
    }
    Ok(Value::Object(body))
}

fn setting_value(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_string)
}

/// One line per operational setting, with its default when it has been overridden. Keys come
/// in the order `serde_json` keeps them, which is alphabetical.
fn settings_text(data: &Value) -> String {
    let Some(operational) = data["operational"].as_object() else {
        return String::new();
    };
    operational
        .iter()
        .map(|(key, setting)| {
            let default = if setting["overridden"] == true {
                format!(" (default {})", setting_value(&setting["default"]))
            } else {
                String::new()
            };
            format!("{key}: {}{default}\n", setting_value(&setting["value"]))
        })
        .collect()
}

/// Shows the shared settings, after changing them when pairs or resets are given.
pub(crate) async fn set(
    client: &ApiClient,
    pairs: &[String],
    resets: &[String],
    json: bool,
) -> anyhow::Result<()> {
    let data = if pairs.is_empty() && resets.is_empty() {
        client.get("/settings").await?
    } else {
        client
            .put_json("/settings", &settings_body(pairs, resets)?)
            .await?
    };
    if json {
        print_json(&data)
    } else {
        print!("{}", settings_text(&data));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_stream_reads_as_one_line() {
        let row = json!({
            "id": 1,
            "video_id": "abcDEF123_-",
            "state": "error",
            "state_reason": "offline",
            "period": 42244.71,
            "follow_sq": 7,
            "open_fragments": 12,
            "lost_fragments": 0,
            "title": "Example TV",
        });

        assert_eq!(
            stream_line(&row),
            "1 abcDEF123_- error (offline), 42244.7 s, next sq 7, 12 to fill, 0 lost: Example TV\n"
        );
    }

    #[test]
    fn a_stream_with_no_period_says_so() {
        let row = json!({ "id": 1, "period": null, "follow_sq": null });

        assert!(stream_line(&row).contains("no period"));
    }

    #[test]
    fn a_failed_job_shows_why() {
        let row = json!({ "id": 2, "state": "failed", "kind": "sweep", "error": "no disk" });

        assert_eq!(job_line(&row, 0), "2 failed sweep: no disk\n");
    }

    #[test]
    fn a_running_job_shows_how_far_it_is_and_what_is_left() {
        let row = json!({
            "id": 2,
            "state": "running",
            "kind": "backfill",
            "progress": 0.25,
            "started_at": 0,
        });

        // A quarter took 30 minutes, so three quarters take 90.
        assert_eq!(
            job_line(&row, 30 * MINUTE_MS),
            "2 running backfill 25%, about 1 h 30 min left\n"
        );
    }

    #[test]
    fn a_job_that_has_not_moved_shows_no_estimate() {
        let row = json!({
            "id": 2,
            "state": "running",
            "kind": "features",
            "progress": 0.0,
            "started_at": 0,
        });

        assert_eq!(job_line(&row, 30 * MINUTE_MS), "2 running features\n");
    }

    #[test]
    fn a_short_wait_reads_in_minutes() {
        assert_eq!(
            left(0.5, 0, 10 * MINUTE_MS),
            Some("about 10 min left".to_string())
        );
        assert_eq!(
            left(0.99, 0, 10 * MINUTE_MS),
            Some("about 1 min left".to_string())
        );
        assert_eq!(left(0.5, 0, -1), None);
    }

    #[test]
    fn a_change_reads_as_one_line() {
        let row = json!({ "id": 3, "state": "confirmed", "stream_id": 1, "start_time": 5.0 });

        assert_eq!(change_line(&row), "3 confirmed stream 1, from 5.0\n");
    }

    #[test]
    fn a_backfill_body_carries_hours_only_when_given() {
        let request = JobRequest::Backfill {
            stream: "abcDEF123_-",
            hours: None,
        };
        let with_hours = JobRequest::Backfill {
            stream: "abcDEF123_-",
            hours: Some(6.5),
        };

        assert_eq!(
            job_body(&request, Some(4)),
            json!({ "kind": "backfill", "stream": 4 })
        );
        assert_eq!(
            job_body(&with_hours, Some(4)),
            json!({ "kind": "backfill", "stream": 4, "hours": 6.5 })
        );
    }

    #[test]
    fn a_features_body_names_the_stream() {
        let request = JobRequest::Features {
            stream: "abcDEF123_-",
        };

        assert_eq!(
            job_body(&request, Some(4)),
            json!({ "kind": "features", "stream": 4 })
        );
    }

    #[test]
    fn a_sweep_body_names_no_stream() {
        assert_eq!(
            job_body(&JobRequest::Sweep, None),
            json!({ "kind": "sweep" })
        );
    }

    #[test]
    fn a_fetch_body_carries_its_range() {
        let request = JobRequest::Fetch {
            stream: "abcDEF123_-",
            itag: 140,
            first: 10,
            last: 20,
        };

        assert_eq!(
            job_body(&request, Some(4)),
            json!({ "kind": "fetch", "stream": 4, "itag": 140, "first": 10, "last": 20 })
        );
    }

    #[test]
    fn a_number_in_a_setting_stays_a_number() {
        assert_eq!(
            parse_setting("concurrency=2").unwrap(),
            ("concurrency".to_string(), json!(2))
        );
        assert_eq!(
            parse_setting("requests_per_second=2.5").unwrap(),
            ("requests_per_second".to_string(), json!(2.5))
        );
    }

    #[test]
    fn text_in_a_setting_becomes_a_string() {
        assert_eq!(
            parse_setting("daily_summary=09:00").unwrap(),
            ("daily_summary".to_string(), json!("09:00"))
        );
        assert_eq!(
            parse_setting("daily_summary=off").unwrap(),
            ("daily_summary".to_string(), json!("off"))
        );
    }

    #[test]
    fn a_setting_without_an_equals_sign_is_refused() {
        let error = parse_setting("concurrency").unwrap_err();

        assert_eq!(error.to_string(), "expected KEY=VALUE, got \"concurrency\"");
        assert!(parse_setting("=2").is_err());
    }

    #[test]
    fn pairs_set_and_resets_drop_overrides() {
        let pairs = ["concurrency=2".to_string(), "daily_summary=off".to_string()];
        let resets = ["requests_per_second".to_string()];

        assert_eq!(
            settings_body(&pairs, &resets).unwrap(),
            json!({ "concurrency": 2, "daily_summary": "off", "requests_per_second": null })
        );
    }

    #[test]
    fn a_bad_pair_fails_the_whole_body() {
        assert!(settings_body(&["nonsense".to_string()], &[]).is_err());
    }

    #[test]
    fn settings_show_their_default_only_when_overridden() {
        let data = json!({
            "operational": {
                "concurrency": { "value": 2, "default": 4, "overridden": true },
                "requests_per_second": { "value": 4.0, "default": 4.0, "overridden": false },
                "daily_summary": { "value": "09:00", "default": "off", "overridden": true },
            },
            "bootstrap": {},
        });

        assert_eq!(
            settings_text(&data),
            "concurrency: 2 (default 4)\ndaily_summary: 09:00 (default off)\nrequests_per_second: 4.0\n"
        );
    }

    fn doctor_data(probe: Value) -> Value {
        json!({
            "doctor": {
                "tools": [],
                "has_ipv6_route": true,
                "limits": { "concurrency": 4, "requests_per_second": 4.0 },
                "policy": "v6-first",
                "active_family": "v6",
                "last_switch": null,
                "resolution": [],
            },
            "probe": probe,
        })
    }

    #[test]
    fn a_doctor_with_no_probe_shows_only_the_report() {
        let text = doctor_output(&doctor_data(Value::Null)).unwrap();

        assert!(text.contains("ipv6 route: true"));
        assert!(!text.contains("probe"));
    }

    #[test]
    fn a_failed_probe_shows_its_message() {
        let text = doctor_output(&doctor_data(json!({ "error": "no formats" }))).unwrap();

        assert!(text.ends_with("probe: no formats\n"));
    }

    #[test]
    fn a_stream_is_found_by_its_video_id() {
        let data = json!([{ "id": 4, "video_id": "abcDEF123_-" }]);

        assert_eq!(row_id(&data, "abcDEF123_-"), Some(4));
        assert_eq!(row_id(&data, "zzzZZZ999_-"), None);
    }
}

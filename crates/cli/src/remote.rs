//! Commands that go through the running service (FR-C1).

use anyhow::anyhow;
use dalsegno_app::acquisition::video_id_of;
use dalsegno_app::remote::ApiClient;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

/// As many rows as one page of the API holds.
const PAGE: &str = "?limit=200";
const MINUTE_MS: i64 = 60_000;
const MINUTES_PER_HOUR: i64 = 60;

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
    let next = row["follow_sq"]
        .as_u64()
        .map_or("-".to_string(), |sq| sq.to_string());
    format!(
        "{} {} {}{reason}, {}, next sq {}: {}\n",
        row["id"],
        text(row, "video_id"),
        text(row, "state"),
        period,
        next,
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

/// Calls `/streams/{id}/{action}` for a video ID or URL, and shows the stream.
pub(crate) async fn stream_action(
    client: &ApiClient,
    stream: &str,
    action: &str,
    json: bool,
) -> anyhow::Result<()> {
    let video_id = video_id_of(stream)?;
    let streams = client.get(&format!("/streams{PAGE}")).await?;
    let id = row_id(&streams, &video_id)
        .ok_or_else(|| anyhow!("stream {video_id} is not registered"))?;
    let data = client.post(&format!("/streams/{id}/{action}")).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&data)?);
    } else {
        print!("{}", stream_line(&data));
    }
    Ok(())
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
            "title": "Example TV",
        });

        assert_eq!(
            stream_line(&row),
            "1 abcDEF123_- error (offline), 42244.7 s, next sq 7: Example TV\n"
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
    fn a_stream_is_found_by_its_video_id() {
        let data = json!([{ "id": 4, "video_id": "abcDEF123_-" }]);

        assert_eq!(row_id(&data, "abcDEF123_-"), Some(4));
        assert_eq!(row_id(&data, "zzzZZZ999_-"), None);
    }
}

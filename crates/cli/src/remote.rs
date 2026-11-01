//! Commands that go through the running service (FR-C1).

use anyhow::anyhow;
use dalsegno_app::acquisition::video_id_of;
use dalsegno_app::remote::ApiClient;
use serde_json::Value;

/// As many rows as one page of the API holds.
const PAGE: &str = "?limit=200";

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

fn job_line(row: &Value) -> String {
    let error = row["error"]
        .as_str()
        .map_or(String::new(), |error| format!(": {error}"));
    format!(
        "{} {} {}{error}\n",
        row["id"],
        text(row, "state"),
        text(row, "kind")
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

fn print_rows(json: bool, data: &Value, line: fn(&Value) -> String) -> anyhow::Result<()> {
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
    print_rows(json, &data, job_line)
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

        assert_eq!(job_line(&row), "2 failed sweep: no disk\n");
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

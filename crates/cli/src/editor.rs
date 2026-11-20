//! The boundary editor's command through the service (FR-E2, FR-E3).

use crate::remote::{print_json, text};
use anyhow::bail;
use dalsegno_app::remote::ApiClient;
use serde_json::Value;
use std::time::Duration;

const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 3_600;
/// How often the job is asked after.
const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// How long the job is waited for.
const POLL_LIMIT: Duration = Duration::from_secs(30 * 60);

/// Where a job stands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum JobProgress {
    Waiting,
    Succeeded,
    /// Failed or cancelled, with the reason to show.
    Ended(String),
}

/// Reads a job's state as `GET /jobs/{id}` gives it.
fn job_progress(job: &Value) -> JobProgress {
    match text(job, "state") {
        "succeeded" => JobProgress::Succeeded,
        "failed" => JobProgress::Ended(
            job["error"]
                .as_str()
                .unwrap_or("the job failed")
                .to_string(),
        ),
        "cancelled" => JobProgress::Ended("the job was cancelled".to_string()),
        _ => JobProgress::Waiting,
    }
}

/// Seconds as `h:mm:ss` with `decimals` places of a second, rounded.
fn clock(seconds: f64, decimals: u32) -> String {
    let scale = 10_u64.pow(decimals);
    let units = (seconds.max(0.0) * scale as f64).round() as u64;
    let (whole, fraction) = (units / scale, units % scale);
    let fraction = if decimals == 0 {
        String::new()
    } else {
        format!(".{fraction:0width$}", width = decimals as usize)
    };
    format!(
        "{}:{:02}:{:02}{fraction}",
        whole / SECONDS_PER_HOUR,
        whole % SECONDS_PER_HOUR / SECONDS_PER_MINUTE,
        whole % SECONDS_PER_MINUTE
    )
}

fn number(row: &Value, key: &str) -> f64 {
    row[key].as_f64().unwrap_or(0.0)
}

fn list<'a>(row: &'a Value, key: &str) -> &'a [Value] {
    row[key].as_array().map_or(&[], Vec::as_slice)
}

/// What a candidate rests on, in words.
fn evidence_text(candidate: &Value) -> &str {
    match text(candidate, "evidence") {
        "both" => "black+silence",
        "black_only" => "black only",
        "silence_only" => "silence only",
        other => other,
    }
}

fn start_line(candidate: &Value) -> String {
    let keyframe = &candidate["keyframe"];
    format!(
        "  start {}  keyframe sq {}  {}\n",
        clock(number(keyframe, "time"), 3),
        keyframe["sq"],
        evidence_text(candidate)
    )
}

fn end_line(candidate: &Value) -> String {
    let edge = match text(candidate, "edge") {
        "sound_stops" => "sound stops",
        "sound_returns" => "sound returns",
        other => other,
    };
    format!(
        "  end {}  {edge}  {}\n",
        clock(number(candidate, "time"), 3),
        evidence_text(candidate)
    )
}

/// One edge: where it was examined, then its candidates, best first.
fn edge_text(label: &str, report: &Value) -> String {
    if !report.is_object() {
        return format!("{label}  no report\n");
    }
    let window = &report["window"];
    let (candidates, line): (&[Value], fn(&Value) -> String) = if label == "start" {
        (list(report, "starts"), start_line)
    } else {
        (list(report, "ends"), end_line)
    };
    let lines: String = candidates.iter().map(line).collect();
    let lines = if lines.is_empty() {
        "  no candidates\n".to_string()
    } else {
        lines
    };
    let relaxed = if report["relaxed"].as_bool().unwrap_or(false) {
        "  silence was loosened to find these\n"
    } else {
        ""
    };
    format!(
        "{label}  around {}  window {}\u{2013}{}\n{lines}{relaxed}",
        clock(number(report, "around"), 1),
        clock(number(window, "start"), 1),
        clock(number(window, "end"), 1),
    )
}

/// The period of a cycle clip, or nothing when none was measured.
fn period_text(period: &Value) -> String {
    if !period.is_object() {
        return String::new();
    }
    let video = &period["video"];
    let video = if video.is_object() {
        format!(
            "video {:.2} s (from {} black intervals)",
            number(video, "period"),
            video["agreeing"]
        )
    } else {
        "video: not found".to_string()
    };
    format!("period  audio {:.1} s  {video}\n", number(period, "audio"))
}

/// Both edges, then the period of a cycle.
pub(crate) fn candidates_text(data: &Value) -> String {
    format!(
        "{}{}{}",
        edge_text("start", &data["start"]),
        edge_text("end", &data["end"]),
        period_text(&data["period"])
    )
}

/// The ID of the job a response waits for, if it does.
fn waiting_job(data: &Value) -> Option<i64> {
    data["job"]["id"].as_i64()
}

/// Waits for a job to leave the queue and finish.
async fn wait_for_job(client: &ApiClient, job: i64) -> anyhow::Result<()> {
    let polls = POLL_LIMIT.as_secs() / POLL_INTERVAL.as_secs();
    for _ in 0..polls {
        match job_progress(&client.get(&format!("/jobs/{job}")).await?) {
            JobProgress::Succeeded => return Ok(()),
            JobProgress::Ended(reason) => bail!("{reason}"),
            JobProgress::Waiting => tokio::time::sleep(POLL_INTERVAL).await,
        }
    }
    bail!(
        "the job did not finish within {} minutes",
        POLL_LIMIT.as_secs() / SECONDS_PER_MINUTE
    )
}

/// Shows where a clip's edges could go, waiting for the service to find them first.
pub(crate) async fn refine(client: &ApiClient, clip: i64, json: bool) -> anyhow::Result<()> {
    let path = format!("/clips/{clip}/candidates");
    let mut data = client.get(&path).await?;
    if let Some(job) = waiting_job(&data) {
        wait_for_job(client, job).await?;
        data = client.get(&path).await?;
        if waiting_job(&data).is_some() {
            bail!("the candidates are still not ready; ask again in a moment");
        }
    }
    if json {
        return print_json(&data);
    }
    print!("{}", candidates_text(&data));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn start_report() -> Value {
        json!({
            "around": 3723.4,
            "window": { "start": 3703.4, "end": 3743.4 },
            "preview_start": 3703.4,
            "keyframes": [{ "sq": 4321, "time": 3720.0 }],
            "black": [], "silence": [],
            "starts": [
                { "keyframe": { "sq": 4321, "time": 3720.0 }, "evidence": "both",
                  "interval": { "start": 3720.5, "end": 3722.0 } },
                { "keyframe": { "sq": 4322, "time": 3725.0 }, "evidence": "silence_only",
                  "interval": { "start": 3725.5, "end": 3726.0 } },
            ],
            "ends": [],
            "relaxed": false,
        })
    }

    #[test]
    fn a_start_edge_lists_its_candidates_best_first() {
        assert_eq!(
            edge_text("start", &start_report()),
            "start  around 1:02:03.4  window 1:01:43.4\u{2013}1:02:23.4\n\
             \x20 start 1:02:00.000  keyframe sq 4321  black+silence\n\
             \x20 start 1:02:05.000  keyframe sq 4322  silence only\n"
        );
    }

    #[test]
    fn an_end_edge_names_the_edge_of_the_silence_and_a_relaxed_search() {
        let report = json!({
            "around": 100.0, "window": { "start": 80.0, "end": 120.0 },
            "starts": [], "relaxed": true,
            "ends": [{ "time": 101.25, "edge": "sound_returns", "evidence": "black_only" }],
        });

        assert_eq!(
            edge_text("end", &report),
            "end  around 0:01:40.0  window 0:01:20.0\u{2013}0:02:00.0\n\
             \x20 end 0:01:41.250  sound returns  black only\n\
             \x20 silence was loosened to find these\n"
        );
    }

    #[test]
    fn an_edge_without_candidates_or_report_says_so() {
        let mut report = start_report();
        report["starts"] = json!([]);

        assert!(edge_text("start", &report).ends_with("\n  no candidates\n"));
        assert_eq!(edge_text("end", &Value::Null), "end  no report\n");
    }

    #[test]
    fn a_period_shows_audio_and_video() {
        let found = json!({
            "start": 1.0, "audio": 3600.0, "video": { "period": 3599.96, "agreeing": 2 },
        });
        let missing = json!({ "start": 1.0, "audio": 3600.0, "video": null });

        assert_eq!(
            period_text(&found),
            "period  audio 3600.0 s  video 3599.96 s (from 2 black intervals)\n"
        );
        assert_eq!(
            period_text(&missing),
            "period  audio 3600.0 s  video: not found\n"
        );
        assert_eq!(period_text(&Value::Null), "");
    }

    #[test]
    fn a_job_is_waited_for_until_it_ends() {
        let state =
            |state: &str, error: Value| job_progress(&json!({ "state": state, "error": error }));

        assert_eq!(state("queued", Value::Null), JobProgress::Waiting);
        assert_eq!(state("running", Value::Null), JobProgress::Waiting);
        assert_eq!(state("succeeded", Value::Null), JobProgress::Succeeded);
        assert_eq!(
            state("failed", json!("no stored audio")),
            JobProgress::Ended("no stored audio".to_string())
        );
        assert_eq!(
            state("cancelled", Value::Null),
            JobProgress::Ended("the job was cancelled".to_string())
        );
    }

    #[test]
    fn a_response_waits_only_when_it_names_a_job() {
        assert_eq!(waiting_job(&json!({ "job": { "id": 7 } })), Some(7));
        assert_eq!(waiting_job(&json!({ "job": null })), None);
    }
}

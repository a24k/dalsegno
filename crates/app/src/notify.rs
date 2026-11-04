//! The words of a notification. UI text and notifications are English (D-28).

use chrono::DateTime;
use chrono_tz::Asia::Tokyo;
use dalsegno_io::webhook::{Link, Message, Tone};
use dalsegno_store::notifications::Notification;
use serde_json::Value;

/// The kind of a notification about a change.
pub const CHANGE: &str = "change";
/// The kind of a notification about a DVR deadline.
pub const DEADLINE: &str = "deadline";
/// The kind of a notification about a stream.
pub const STREAM: &str = "stream";
/// The kind of a notification about a hole that cannot be filled.
pub const GAP: &str = "gap";
/// The kind of a notification about the service itself.
pub const SERVICE: &str = "service";
/// The kind of the daily summary.
pub const SUMMARY: &str = "summary";

const HOUR_MS: f64 = 3_600_000.0;
/// How a time is shown: month, day and time in Japan, where the operator is.
const TIME_FORMAT: &str = "%m/%d %H:%M %Z";

fn number(body: &Value, key: &str) -> f64 {
    body[key].as_f64().unwrap_or(0.0)
}

fn text<'a>(body: &'a Value, key: &str) -> &'a str {
    body[key].as_str().unwrap_or("unknown")
}

/// Counts a noun, so that one job is not "1 jobs".
fn count(amount: f64, noun: &str) -> String {
    let suffix = if amount == 1.0 { "" } else { "s" };
    format!("{amount:.0} {noun}{suffix}")
}

fn capitalised(words: &str) -> String {
    let mut characters = words.chars();
    characters.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(characters).collect()
    })
}

fn time_left(deadline_ms: f64, now: i64) -> String {
    let hours = (deadline_ms - now as f64) / HOUR_MS;
    if hours <= 0.0 {
        "no time left".to_string()
    } else if hours < 1.0 {
        "under an hour left".to_string()
    } else {
        format!("{} left", count(hours.round(), "hour"))
    }
}

/// A time as the operator reads it.
fn in_japan(ms: f64) -> Option<String> {
    DateTime::from_timestamp_millis(ms as i64)
        .map(|at| at.with_timezone(&Tokyo).format(TIME_FORMAT).to_string())
}

/// When the old version must be exported by, as date, time and hours left.
fn by_deadline(body: &Value, now: i64) -> String {
    let deadline_ms = body["dvr_deadline_at"].as_f64();
    let when = deadline_ms.and_then(in_japan);
    match (deadline_ms, when) {
        (Some(ms), Some(when)) => format!(" by {when} ({})", time_left(ms, now)),
        _ => String::new(),
    }
}

fn change_id(subject: &str) -> Option<&str> {
    subject.strip_prefix("change:")?.split(':').next()
}

fn link(notification: &Notification, public_url: Option<&str>) -> Option<Link> {
    let base = public_url?.trim_end_matches('/');
    let (label, path) = match change_id(&notification.subject) {
        Some(id) => ("Open the change", format!("changes/{id}")),
        None if notification.kind == SERVICE => ("Open the jobs", "jobs".to_string()),
        None if notification.kind == SUMMARY => ("Open the dashboard", String::new()),
        None => ("Open the streams", "streams".to_string()),
    };
    Some(Link {
        label: label.to_string(),
        url: format!("{base}/{path}"),
    })
}

fn describe_change(notification: &Notification, body: &Value, now: i64) -> (String, String, Tone) {
    if notification.subject.ends_with(":dismissed") {
        return (
            "Change dismissed".to_string(),
            "A whole cycle matched the old version, so there is nothing to export.".to_string(),
            Tone::Good,
        );
    }
    if notification.subject.ends_with(":confirmed") {
        return (
            "Change confirmed".to_string(),
            format!(
                "The period went from {:.1} s to {:.1} s, with {} compared.",
                number(body, "old_period"),
                number(body, "new_period"),
                count(number(body, "regions"), "region"),
            ),
            Tone::Notice,
        );
    }
    // The first signal can be audio that matches nothing, which has no length yet.
    let so_far = body["delta"].as_f64().map_or_else(
        || "Its length is not measured yet.".to_string(),
        |delta| format!("{delta:+.1} s so far."),
    );
    (
        "Change seen".to_string(),
        format!(
            "{so_far} Export the old version{} before it leaves the DVR.",
            by_deadline(body, now),
        ),
        Tone::Notice,
    )
}

/// What a stream notification is about and how serious, from the event its subject names.
fn stream_event(subject: &str) -> (&'static str, Tone) {
    match subject.split(':').nth(2) {
        Some("ended") => ("Stream ended", Tone::Notice),
        Some("error") => ("Stream error", Tone::Bad),
        Some("recovered") => ("Stream recovered", Tone::Good),
        _ => ("Stream", Tone::Notice),
    }
}

fn describe_start(body: &Value) -> (String, String) {
    let requeued = number(body, "requeued");
    let jobs = if requeued == 0.0 {
        "No interrupted jobs to requeue.".to_string()
    } else {
        format!("Requeued {}.", count(requeued, "interrupted job"))
    };
    let ignored = body["ignored"].as_array().map_or(&[][..], Vec::as_slice);
    let problems = ignored
        .iter()
        .filter_map(Value::as_str)
        .map(|problem| format!("{}.", capitalised(problem)));
    let details = [jobs]
        .into_iter()
        .chain(problems)
        .collect::<Vec<_>>()
        .join("\n");
    (
        format!("Service started: dalsegno {}", text(body, "version")),
        details,
    )
}

/// One line for a stream in the summary.
fn summary_line(stream: &Value) -> String {
    let period = stream["period"].as_f64().map_or_else(
        || "no period yet".to_string(),
        |period| format!("period {period:.1} s"),
    );
    format!(
        "- {}: {}, {period}, {} to fill, {} lost.",
        text(stream, "stream").replace('\n', " "),
        text(stream, "state"),
        count(number(stream, "open"), "fragment"),
        number(stream, "lost"),
    )
}

fn describe_summary(body: &Value) -> (String, String) {
    let streams = body["streams"].as_array().map_or(&[][..], Vec::as_slice);
    let failed = body["failed"].as_array().map_or(&[][..], Vec::as_slice);
    let since = in_japan(number(body, "since")).unwrap_or_else(|| "unknown".to_string());
    let total: f64 = failed.iter().map(|kind| number(kind, "count")).sum();
    let failures = if failed.is_empty() {
        format!("No jobs failed since {since}.")
    } else {
        let kinds: Vec<String> = failed
            .iter()
            .map(|kind| format!("{} {}", text(kind, "kind"), number(kind, "count")))
            .collect();
        format!(
            "{} failed since {since}: {}.",
            count(total, "job"),
            kinds.join(", ")
        )
    };
    let lines: Vec<String> = if streams.is_empty() {
        vec!["No streams are being recorded.".to_string()]
    } else {
        streams.iter().map(summary_line).collect()
    };
    let details = lines
        .into_iter()
        .chain([failures])
        .collect::<Vec<_>>()
        .join("\n");
    ("Daily summary".to_string(), details)
}

/// Words a notification: what happened and to which stream, the details, how serious it is,
/// and a link when a public URL is set.
pub fn render(notification: &Notification, public_url: Option<&str>, now: i64) -> Message {
    let body: Value = serde_json::from_str(&notification.body).unwrap_or(Value::Null);
    let stream = Some(text(&body, "stream").to_string());
    let ((event, details), subject, tone) = match notification.kind.as_str() {
        CHANGE => {
            let (event, details, tone) = describe_change(notification, &body, now);
            ((event, details), stream, tone)
        }
        DEADLINE => (
            (
                "DVR deadline".to_string(),
                format!(
                    "Export the old version{} before it leaves the DVR.",
                    by_deadline(&body, now),
                ),
            ),
            stream,
            Tone::Notice,
        ),
        STREAM => {
            let (event, tone) = stream_event(&notification.subject);
            let words = (event.to_string(), capitalised(text(&body, "message")));
            (words, stream, tone)
        }
        GAP => (
            (
                "Lost gap".to_string(),
                format!(
                    "{:.0} s of track {} left the DVR before it was fetched, so the recording \
                     has a hole that cannot be filled.",
                    number(&body, "seconds"),
                    number(&body, "itag"),
                ),
            ),
            stream,
            Tone::Bad,
        ),
        SERVICE => (describe_start(&body), None, start_tone(&body)),
        SUMMARY => (describe_summary(&body), None, summary_tone(&body)),
        other => (
            (other.to_string(), notification.subject.clone()),
            None,
            Tone::Notice,
        ),
    };
    Message {
        event,
        subject,
        text: details,
        link: link(notification, public_url),
        tone,
    }
}

fn has_items(body: &Value, key: &str) -> bool {
    body[key].as_array().is_some_and(|items| !items.is_empty())
}

/// A start is good unless it left a setting off.
fn start_tone(body: &Value) -> Tone {
    if has_items(body, "ignored") {
        Tone::Notice
    } else {
        Tone::Good
    }
}

/// A summary is good unless a job failed or a fragment was lost.
fn summary_tone(body: &Value) -> Tone {
    let streams = body["streams"].as_array().map_or(&[][..], Vec::as_slice);
    let has_lost = streams.iter().any(|stream| number(stream, "lost") > 0.0);
    if has_items(body, "failed") || has_lost {
        Tone::Notice
    } else {
        Tone::Good
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 11/04 22:00 in Japan.
    const DEADLINE_MS: i64 = 1_793_797_200_000;
    const NOW: i64 = DEADLINE_MS - 48 * 3_600_000;

    fn notification(kind: &str, subject: &str, body: Value) -> Notification {
        Notification {
            id: 1,
            kind: kind.to_string(),
            subject: subject.to_string(),
            body: body.to_string(),
        }
    }

    fn provisional(deadline_ms: i64) -> Notification {
        let body = json!({
            "stream": "Example TV",
            "delta": 455.0,
            "dvr_deadline_at": deadline_ms,
        });
        notification(CHANGE, "change:7:provisional", body)
    }

    #[test]
    fn the_test_deadline_is_22_00_in_japan() {
        let at = DateTime::parse_from_rfc3339("2026-11-04T22:00:00+09:00").unwrap();

        assert_eq!(at.timestamp_millis(), DEADLINE_MS);
    }

    #[test]
    fn a_provisional_change_gives_the_deadline_and_the_time_left() {
        let message = render(&provisional(DEADLINE_MS), None, NOW);

        assert_eq!(message.title(), "Change seen: Example TV");
        assert_eq!(
            message.text,
            "+455.0 s so far. Export the old version by 11/04 22:00 JST (48 hours left) before \
             it leaves the DVR."
        );
    }

    #[test]
    fn the_last_hour_is_not_rounded_away() {
        let one_hour = render(&provisional(NOW + 3_600_000), None, NOW);
        let minutes = render(&provisional(NOW + 600_000), None, NOW);
        let passed = render(&provisional(NOW - 1), None, NOW);

        assert!(one_hour.text.contains("(1 hour left)"), "{}", one_hour.text);
        assert!(
            minutes.text.contains("(under an hour left)"),
            "{}",
            minutes.text
        );
        assert!(passed.text.contains("(no time left)"), "{}", passed.text);
    }

    #[test]
    fn a_confirmed_change_gives_both_periods() {
        let body = json!({
            "stream": "Example TV",
            "old_period": 100.0,
            "new_period": 150.5,
            "regions": 1,
        });

        let message = render(&notification(CHANGE, "change:7:confirmed", body), None, NOW);

        assert_eq!(message.title(), "Change confirmed: Example TV");
        assert_eq!(
            message.text,
            "The period went from 100.0 s to 150.5 s, with 1 region compared."
        );
    }

    #[test]
    fn a_change_with_no_length_yet_says_so() {
        let body = json!({ "stream": "Example TV", "delta": null, "dvr_deadline_at": DEADLINE_MS });

        let message = render(
            &notification(CHANGE, "change:7:provisional", body),
            None,
            NOW,
        );

        assert!(
            message
                .text
                .starts_with("Its length is not measured yet. Export"),
            "{}",
            message.text
        );
    }

    #[test]
    fn a_dismissed_change_is_good_news() {
        let body = json!({ "stream": "Example TV" });

        let message = render(&notification(CHANGE, "change:7:dismissed", body), None, NOW);

        assert_eq!(message.title(), "Change dismissed: Example TV");
        assert_eq!(message.tone, Tone::Good);
        assert!(
            message.text.contains("nothing to export"),
            "{}",
            message.text
        );
    }

    #[test]
    fn a_change_links_to_its_screen_in_words() {
        let message = render(
            &notification(CHANGE, "change:7:confirmed", json!({})),
            Some("https://dalsegno.example/"),
            NOW,
        );

        assert_eq!(
            message.link,
            Some(Link {
                label: "Open the change".to_string(),
                url: "https://dalsegno.example/changes/7".to_string(),
            })
        );
    }

    #[test]
    fn a_deadline_gives_the_date_and_the_hours_left() {
        let body = json!({ "stream": "Example TV", "dvr_deadline_at": DEADLINE_MS });

        let message = render(
            &notification(DEADLINE, "change:7:deadline", body),
            Some("https://dalsegno.example"),
            NOW,
        );

        assert_eq!(message.title(), "DVR deadline: Example TV");
        assert_eq!(
            message.text,
            "Export the old version by 11/04 22:00 JST (48 hours left) before it leaves the DVR."
        );
        assert_eq!(message.link.unwrap().label, "Open the change");
    }

    #[test]
    fn a_deadline_without_a_time_still_reads() {
        let body = json!({ "stream": "Example TV" });

        let message = render(
            &notification(DEADLINE, "change:7:deadline", body),
            None,
            NOW,
        );

        assert_eq!(
            message.text,
            "Export the old version before it leaves the DVR."
        );
    }

    #[test]
    fn a_stream_says_what_happened_and_links_to_the_streams() {
        let body = json!({ "stream": "Example TV", "message": "following failed: offline" });

        let message = render(
            &notification(STREAM, "stream:1:error:5", body),
            Some("https://dalsegno.example"),
            NOW,
        );

        assert_eq!(message.title(), "Stream error: Example TV");
        assert_eq!(message.text, "Following failed: offline");
        assert_eq!(
            message.link.unwrap().url,
            "https://dalsegno.example/streams"
        );
    }

    #[test]
    fn a_recovery_and_an_end_are_told_apart() {
        let body = json!({ "stream": "Example TV", "message": "x" });

        let recovered = render(
            &notification(STREAM, "stream:1:recovered:5", body.clone()),
            None,
            NOW,
        );
        let ended = render(&notification(STREAM, "stream:1:ended", body), None, NOW);

        assert_eq!(recovered.title(), "Stream recovered: Example TV");
        assert_eq!(ended.title(), "Stream ended: Example TV");
    }

    #[test]
    fn a_lost_gap_says_how_much_is_gone() {
        let body = json!({ "stream": "Example TV", "itag": 140, "seconds": 25.0 });

        let message = render(&notification(GAP, "gap:3:lost", body), None, NOW);

        assert_eq!(message.title(), "Lost gap: Example TV");
        assert!(
            message.text.starts_with("25 s of track 140"),
            "{}",
            message.text
        );
        assert!(
            message.text.contains("cannot be filled"),
            "{}",
            message.text
        );
    }

    #[test]
    fn a_start_gives_the_version_and_links_to_the_jobs() {
        let body = json!({ "version": "0.5.0", "requeued": 2 });

        let message = render(
            &notification(SERVICE, "service:started:1", body),
            Some("https://dalsegno.example"),
            NOW,
        );

        assert_eq!(message.title(), "Service started: dalsegno 0.5.0");
        assert_eq!(message.text, "Requeued 2 interrupted jobs.");
        assert_eq!(message.link.unwrap().label, "Open the jobs");
    }

    #[test]
    fn a_start_says_which_settings_it_left_off() {
        let body = json!({
            "version": "0.5.0",
            "requeued": 0,
            "ignored": ["the daily summary is off"],
        });

        let message = render(&notification(SERVICE, "service:started:1", body), None, NOW);

        assert_eq!(
            message.text,
            "No interrupted jobs to requeue.\nThe daily summary is off."
        );
    }

    #[test]
    fn a_start_counts_one_job_and_none() {
        let start = |requeued: u32| {
            let body = json!({ "version": "0.5.0", "requeued": requeued });
            render(&notification(SERVICE, "service:started:1", body), None, NOW).text
        };

        assert_eq!(start(1), "Requeued 1 interrupted job.");
        assert_eq!(start(0), "No interrupted jobs to requeue.");
    }

    #[test]
    fn a_summary_gives_a_line_per_stream_and_the_failures() {
        let body = json!({
            "since": DEADLINE_MS,
            "streams": [
                { "stream": "Example TV", "state": "recording", "period": 42244.7,
                  "open": 1, "lost": 0 },
                { "stream": "Example Radio", "state": "error", "period": null,
                  "open": 3, "lost": 2 },
            ],
            "failed": [{ "kind": "analysis", "count": 1 }, { "kind": "backfill", "count": 2 }],
        });

        let message = render(
            &notification(SUMMARY, "summary:2026-11-05", body),
            Some("https://dalsegno.example"),
            NOW,
        );

        assert_eq!(message.title(), "Daily summary");
        assert_eq!(
            message.text,
            "- Example TV: recording, period 42244.7 s, 1 fragment to fill, 0 lost.\n\
             - Example Radio: error, no period yet, 3 fragments to fill, 2 lost.\n\
             3 jobs failed since 11/04 22:00 JST: analysis 1, backfill 2."
        );
        let link = message.link.unwrap();
        assert_eq!(link.label, "Open the dashboard");
        assert_eq!(link.url, "https://dalsegno.example/");
    }

    #[test]
    fn a_title_cannot_add_a_line_to_the_summary() {
        let body = json!({
            "since": DEADLINE_MS,
            "streams": [{ "stream": "Example\n- TV", "state": "recording" }],
            "failed": [],
        });

        let message = render(
            &notification(SUMMARY, "summary:2026-11-05", body),
            None,
            NOW,
        );

        assert!(
            message.text.starts_with("- Example - TV: recording"),
            "{}",
            message.text
        );
        assert_eq!(message.text.lines().count(), 2);
    }

    #[test]
    fn a_quiet_summary_says_so() {
        let body = json!({ "since": DEADLINE_MS, "streams": [], "failed": [] });

        let message = render(
            &notification(SUMMARY, "summary:2026-11-05", body),
            None,
            NOW,
        );

        assert_eq!(
            message.text,
            "No streams are being recorded.\nNo jobs failed since 11/04 22:00 JST."
        );
    }

    #[test]
    fn the_stream_is_kept_apart_from_what_happened() {
        let body = json!({ "stream": "Example TV", "itag": 140, "seconds": 25.0 });

        let message = render(&notification(GAP, "gap:3:lost", body), None, NOW);

        assert_eq!(message.event, "Lost gap");
        assert_eq!(message.subject.as_deref(), Some("Example TV"));
        assert_eq!(message.tone, Tone::Bad);
    }

    #[test]
    fn the_tone_follows_how_serious_it_is() {
        let tone = |kind: &str, subject: &str, body: Value| {
            render(&notification(kind, subject, body), None, NOW).tone
        };
        let quiet = json!({ "streams": [{ "lost": 0 }], "failed": [] });
        let lossy = json!({ "streams": [{ "lost": 2 }], "failed": [] });

        assert_eq!(tone(STREAM, "stream:1:recovered:5", json!({})), Tone::Good);
        assert_eq!(tone(STREAM, "stream:1:error:5", json!({})), Tone::Bad);
        assert_eq!(
            tone(CHANGE, "change:7:provisional", json!({})),
            Tone::Notice
        );
        assert_eq!(tone(SERVICE, "service:started:1", json!({})), Tone::Good);
        let ignored = json!({ "ignored": ["x"] });
        assert_eq!(tone(SERVICE, "service:started:1", ignored), Tone::Notice);
        assert_eq!(tone(SUMMARY, "summary:2026-11-05", quiet), Tone::Good);
        assert_eq!(tone(SUMMARY, "summary:2026-11-05", lossy), Tone::Notice);
    }

    #[test]
    fn a_start_and_a_summary_are_about_no_stream() {
        let start = render(
            &notification(SERVICE, "service:started:1", json!({ "version": "0.5.0" })),
            None,
            NOW,
        );

        assert_eq!(start.subject, None);
        assert_eq!(start.title(), "Service started: dalsegno 0.5.0");
    }

    #[test]
    fn an_unknown_kind_still_says_something() {
        let message = render(&notification("other", "x:1", json!({})), None, NOW);

        assert_eq!(
            (message.title().as_str(), message.text.as_str()),
            ("other", "x:1")
        );
        assert_eq!(message.link, None);
    }

    #[test]
    fn a_broken_body_still_says_something() {
        let broken = Notification {
            body: "{".to_string(),
            ..notification(STREAM, "stream:1:ended", json!({}))
        };

        let message = render(&broken, None, NOW);

        assert_eq!(message.title(), "Stream ended: unknown");
    }
}

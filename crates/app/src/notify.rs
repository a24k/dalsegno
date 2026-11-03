//! The words of a notification. UI text and notifications are English (D-28).

use chrono::DateTime;
use chrono_tz::Asia::Tokyo;
use dalsegno_io::webhook::{Link, Message};
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

const HOUR_MS: f64 = 3_600_000.0;
/// How a deadline is shown: month, day and time in Japan, where the operator is.
const DEADLINE_FORMAT: &str = "%m/%d %H:%M %Z";

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

/// When the old version must be exported by, as date, time and hours left.
fn by_deadline(body: &Value, now: i64) -> String {
    let deadline_ms = body["dvr_deadline_at"].as_f64();
    let when = deadline_ms
        .and_then(|ms| DateTime::from_timestamp_millis(ms as i64))
        .map(|at| at.with_timezone(&Tokyo).format(DEADLINE_FORMAT));
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
        None => ("Open the streams", "streams".to_string()),
    };
    Some(Link {
        label: label.to_string(),
        url: format!("{base}/{path}"),
    })
}

fn describe_change(notification: &Notification, body: &Value, now: i64) -> (String, String) {
    let stream = text(body, "stream");
    if notification.subject.ends_with(":confirmed") {
        return (
            format!("Change confirmed: {stream}"),
            format!(
                "The period went from {:.1} s to {:.1} s, with {} compared.",
                number(body, "old_period"),
                number(body, "new_period"),
                count(number(body, "regions"), "region"),
            ),
        );
    }
    (
        format!("Change seen: {stream}"),
        format!(
            "{:+.1} s so far. Export the old version{} before it leaves the DVR.",
            number(body, "delta"),
            by_deadline(body, now),
        ),
    )
}

/// What a stream notification is about, from the event its subject names.
fn stream_event(subject: &str) -> &'static str {
    match subject.split(':').nth(2) {
        Some("ended") => "Stream ended",
        Some("error") => "Stream error",
        Some("recovered") => "Stream recovered",
        _ => "Stream",
    }
}

fn describe_start(body: &Value) -> (String, String) {
    let requeued = number(body, "requeued");
    let details = if requeued == 0.0 {
        "No interrupted jobs to requeue.".to_string()
    } else {
        format!("Requeued {}.", count(requeued, "interrupted job"))
    };
    (
        format!("Service started: dalsegno {}", text(body, "version")),
        details,
    )
}

/// Words a notification: what happened and to what, the details, and a link when a public
/// URL is set.
pub fn render(notification: &Notification, public_url: Option<&str>, now: i64) -> Message {
    let body: Value = serde_json::from_str(&notification.body).unwrap_or(Value::Null);
    let stream = text(&body, "stream");
    let (title, details) = match notification.kind.as_str() {
        CHANGE => describe_change(notification, &body, now),
        DEADLINE => (
            format!("DVR deadline: {stream}"),
            format!(
                "Export the old version{} before it leaves the DVR.",
                by_deadline(&body, now),
            ),
        ),
        STREAM => (
            format!("{}: {stream}", stream_event(&notification.subject)),
            capitalised(text(&body, "message")),
        ),
        GAP => (
            format!("Lost gap: {stream}"),
            format!(
                "{:.0} s of track {} left the DVR before it was fetched, so the recording has a \
                 hole that cannot be filled.",
                number(&body, "seconds"),
                number(&body, "itag"),
            ),
        ),
        SERVICE => describe_start(&body),
        other => (other.to_string(), notification.subject.clone()),
    };
    Message {
        title,
        text: details,
        link: link(notification, public_url),
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

        assert_eq!(message.title, "Change seen: Example TV");
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

        assert_eq!(message.title, "Change confirmed: Example TV");
        assert_eq!(
            message.text,
            "The period went from 100.0 s to 150.5 s, with 1 region compared."
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

        assert_eq!(message.title, "DVR deadline: Example TV");
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

        assert_eq!(message.title, "Stream error: Example TV");
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

        assert_eq!(recovered.title, "Stream recovered: Example TV");
        assert_eq!(ended.title, "Stream ended: Example TV");
    }

    #[test]
    fn a_lost_gap_says_how_much_is_gone() {
        let body = json!({ "stream": "Example TV", "itag": 140, "seconds": 25.0 });

        let message = render(&notification(GAP, "gap:3:lost", body), None, NOW);

        assert_eq!(message.title, "Lost gap: Example TV");
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

        assert_eq!(message.title, "Service started: dalsegno 0.5.0");
        assert_eq!(message.text, "Requeued 2 interrupted jobs.");
        assert_eq!(message.link.unwrap().label, "Open the jobs");
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
    fn an_unknown_kind_still_says_something() {
        let message = render(&notification("other", "x:1", json!({})), None, NOW);

        assert_eq!(
            (message.title.as_str(), message.text.as_str()),
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

        assert_eq!(message.title, "Stream ended: unknown");
    }
}

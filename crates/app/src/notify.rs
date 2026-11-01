//! The words of a notification. UI text and notifications are English (D-28).

use dalsegno_store::notifications::Notification;
use serde_json::Value;

/// The kind of a notification about a change.
pub const CHANGE: &str = "change";
/// The kind of a notification about a DVR deadline.
pub const DEADLINE: &str = "deadline";
/// The kind of a notification about a stream.
pub const STREAM: &str = "stream";

const HOUR_MS: f64 = 3_600_000.0;

fn number(body: &Value, key: &str) -> f64 {
    body[key].as_f64().unwrap_or(0.0)
}

fn text<'a>(body: &'a Value, key: &str) -> &'a str {
    body[key].as_str().unwrap_or("unknown")
}

fn hours_left(body: &Value, now: i64) -> f64 {
    ((number(body, "dvr_deadline_at") - now as f64) / HOUR_MS).max(0.0)
}

fn change_id(subject: &str) -> Option<&str> {
    subject.strip_prefix("change:")?.split(':').next()
}

fn link(notification: &Notification, public_url: Option<&str>) -> Option<String> {
    let base = public_url?.trim_end_matches('/');
    Some(match change_id(&notification.subject) {
        Some(id) => format!("{base}/changes/{id}"),
        None => format!("{base}/streams"),
    })
}

fn describe_change(notification: &Notification, body: &Value, now: i64) -> String {
    let stream = text(body, "stream");
    if notification.subject.ends_with(":confirmed") {
        return format!(
            "{stream}: period changed ({:.1} s -> {:.1} s), {} regions compared.",
            number(body, "old_period"),
            number(body, "new_period"),
            number(body, "regions"),
        );
    }
    format!(
        "{stream}: a change was seen, {:+.1} s so far. Export the old version within {:.0} hours.",
        number(body, "delta"),
        hours_left(body, now),
    )
}

/// Words a notification, with a link when a public URL is set.
pub fn render(notification: &Notification, public_url: Option<&str>, now: i64) -> String {
    let body: Value = serde_json::from_str(&notification.body).unwrap_or(Value::Null);
    let message = match notification.kind.as_str() {
        CHANGE => describe_change(notification, &body, now),
        DEADLINE => format!(
            "{}: export the old version within {:.0} hours, before it leaves the DVR.",
            text(&body, "stream"),
            hours_left(&body, now),
        ),
        STREAM => format!("{}: {}", text(&body, "stream"), text(&body, "message")),
        other => format!("{other}: {}", notification.subject),
    };
    match link(notification, public_url) {
        Some(link) => format!("{message}\n{link}"),
        None => message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_000;

    fn notification(kind: &str, subject: &str, body: Value) -> Notification {
        Notification {
            id: 1,
            kind: kind.to_string(),
            subject: subject.to_string(),
            body: body.to_string(),
        }
    }

    #[test]
    fn a_provisional_change_gives_the_time_left() {
        let body = json!({
            "stream": "Example",
            "delta": 455.0,
            "dvr_deadline_at": NOW as f64 + 48.0 * HOUR_MS,
        });

        let text = render(
            &notification(CHANGE, "change:7:provisional", body),
            None,
            NOW,
        );

        assert!(text.contains("+455.0 s"), "{text}");
        assert!(text.contains("48 hours"), "{text}");
    }

    #[test]
    fn a_confirmed_change_gives_both_periods() {
        let body = json!({
            "stream": "Example",
            "old_period": 100.0,
            "new_period": 150.5,
            "regions": 3,
        });

        let text = render(&notification(CHANGE, "change:7:confirmed", body), None, NOW);

        assert!(text.contains("100.0 s -> 150.5 s"), "{text}");
    }

    #[test]
    fn a_change_links_to_its_screen() {
        let text = render(
            &notification(CHANGE, "change:7:confirmed", json!({})),
            Some("https://dalsegno.example/"),
            NOW,
        );

        assert!(
            text.ends_with("\nhttps://dalsegno.example/changes/7"),
            "{text}"
        );
    }

    #[test]
    fn a_stream_links_to_the_streams() {
        let body = json!({ "stream": "Example", "message": "following failed: offline" });

        let text = render(
            &notification(STREAM, "stream:1:error:5", body),
            Some("https://dalsegno.example"),
            NOW,
        );

        assert!(
            text.starts_with("Example: following failed: offline"),
            "{text}"
        );
        assert!(text.ends_with("/streams"), "{text}");
    }

    #[test]
    fn a_passed_deadline_says_zero() {
        let body = json!({ "stream": "Example", "dvr_deadline_at": 0.0 });

        let text = render(
            &notification(DEADLINE, "change:7:deadline", body),
            None,
            NOW,
        );

        assert!(text.contains("within 0 hours"), "{text}");
    }

    #[test]
    fn a_broken_body_still_says_something() {
        let broken = Notification {
            body: "{".to_string(),
            ..notification(STREAM, "stream:1:ended", json!({}))
        };

        assert!(!render(&broken, None, NOW).is_empty());
    }
}

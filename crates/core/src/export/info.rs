//! The sanitised `info.json` (FR-E9).
//!
//! Written from an allowlist rather than by deleting known keys, so a key that yt-dlp adds later
//! cannot carry the client's IP address into the library.

use serde_json::{Map, Value};

/// The keys that are kept. All describe the stream and none describe its delivery.
const ALLOWED_KEYS: [&str; 15] = [
    "id",
    "title",
    "fulltitle",
    "description",
    "channel",
    "channel_id",
    "channel_url",
    "uploader",
    "uploader_id",
    "uploader_url",
    "webpage_url",
    "upload_date",
    "release_timestamp",
    "categories",
    "tags",
];

/// Keeps the allowed keys of yt-dlp's JSON. Only text, numbers and lists of text survive, so
/// nothing can arrive nested inside an allowed key.
pub fn sanitise_info(info: &Value) -> Value {
    let Some(object) = info.as_object() else {
        return Value::Object(Map::new());
    };
    let kept = ALLOWED_KEYS.iter().filter_map(|key| {
        let value = object.get(*key)?;
        is_plain(value).then(|| ((*key).to_string(), value.clone()))
    });
    Value::Object(kept.collect())
}

fn is_plain(value: &Value) -> bool {
    match value {
        Value::String(_) | Value::Number(_) => true,
        Value::Array(items) => items.iter().all(Value::is_string),
        Value::Null | Value::Bool(_) | Value::Object(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// FR-E9 lists these as carrying the client's address.
    const FORBIDDEN_KEYS: [&str; 6] = [
        "formats",
        "requested_formats",
        "url",
        "manifest_url",
        "fragments",
        "http_headers",
    ];

    fn hostile_info() -> Value {
        let address = "https://example.invalid/videoplayback?ip=203.0.113.7";
        json!({
            "id": "AAAAAAAAAAA",
            "title": "Example TV",
            "formats": [{"url": address}],
            "requested_formats": [{"url": address}],
            "url": address,
            "manifest_url": address,
            "fragments": [{"url": address}],
            "http_headers": {"Cookie": "secret"},
            "a_key_from_a_future_yt_dlp": address,
            "tags": ["one", "two"],
        })
    }

    #[test]
    fn no_forbidden_key_survives() {
        let clean = sanitise_info(&hostile_info());

        for key in FORBIDDEN_KEYS {
            assert!(clean.get(key).is_none(), "{key}");
        }
    }

    #[test]
    fn the_address_appears_nowhere() {
        let clean = sanitise_info(&hostile_info()).to_string();

        assert!(!clean.contains("203.0.113.7"), "{clean}");
    }

    #[test]
    fn unknown_keys_are_dropped() {
        assert!(
            sanitise_info(&hostile_info())
                .get("a_key_from_a_future_yt_dlp")
                .is_none()
        );
    }

    #[test]
    fn descriptive_keys_are_kept() {
        let clean = sanitise_info(&hostile_info());

        assert_eq!(clean["title"], "Example TV");
        assert_eq!(clean["tags"], json!(["one", "two"]));
    }

    #[test]
    fn an_allowed_key_cannot_smuggle_an_object() {
        let info = json!({
            "title": {"url": "https://example.invalid/?ip=203.0.113.7"},
            "tags": [{"url": "https://example.invalid/?ip=203.0.113.7"}],
        });

        assert_eq!(sanitise_info(&info), json!({}));
    }

    #[test]
    fn anything_but_an_object_gives_nothing() {
        assert_eq!(sanitise_info(&json!(["id"])), json!({}));
    }
}

//! Commands that show and shape a change through the service (FR-C1).

use crate::remote::{print_json, text};
use dalsegno_app::remote::ApiClient;
use serde_json::{Map, Value, json};

const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 3_600;

/// What a clip holds.
#[derive(Debug, Clone, Copy, PartialEq, clap::ValueEnum)]
pub(crate) enum ClipKind {
    /// A whole cycle of a version.
    Cycle,
    /// Only what a version added.
    Addition,
}

impl ClipKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cycle => "cycle",
            Self::Addition => "addition",
        }
    }
}

/// What `clip` was asked to do.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ClipAction {
    Create {
        change: i64,
        kind: ClipKind,
        from: f64,
        to: f64,
        version: Option<i64>,
    },
    Show(i64),
    Edit {
        clip: i64,
        kind: Option<ClipKind>,
        from: Option<f64>,
        to: Option<f64>,
    },
}

/// Seconds as `h:mm:ss`.
fn clock(seconds: f64) -> String {
    let whole = seconds.max(0.0).round() as u64;
    format!(
        "{}:{:02}:{:02}",
        whole / SECONDS_PER_HOUR,
        whole % SECONDS_PER_HOUR / SECONDS_PER_MINUTE,
        whole % SECONDS_PER_MINUTE
    )
}

fn number(row: &Value, key: &str) -> f64 {
    row[key].as_f64().unwrap_or(0.0)
}

/// A bar of a comparison, its regions relative to its own start.
fn bar_text(label: &str, bar: &Value) -> String {
    let bar_start = number(bar, "start");
    let regions: String = bar["regions"]
        .as_array()
        .map_or(&[][..], |list| &list[..])
        .iter()
        .map(|region| {
            let (start, end) = (number(region, "start"), number(region, "end"));
            let boundaries: String = region["video_boundaries"]
                .as_array()
                .map_or(&[][..], |list| &list[..])
                .iter()
                .filter_map(Value::as_f64)
                .map(|time| format!("    video boundary at {}\n", clock(time - bar_start)))
                .collect();
            format!(
                "  {}  {}\u{2013}{}  ({:.1} s)\n{boundaries}",
                text(region, "class"),
                clock(start - bar_start),
                clock(end - bar_start),
                end - start,
            )
        })
        .collect();
    format!(
        "{label}  version {}  from {:.1} s  period {:.1} s\n{regions}",
        bar["version_id"],
        bar_start,
        number(bar, "period"),
    )
}

fn check_line(check: &Value) -> String {
    let result = if check["passed"].as_bool().unwrap_or(false) {
        "passed"
    } else {
        "REVIEW"
    };
    format!(
        "  {result}  {}  expected {:.1}  actual {:.1}  \u{b1}{:.1}\n",
        text(check, "name"),
        number(check, "expected"),
        number(check, "actual"),
        number(check, "tolerance"),
    )
}

/// The state of a change, then its bars and checks.
pub(crate) fn comparison_text(data: &Value) -> String {
    let bars: String = [("old", &data["old"]), ("new", &data["new"])]
        .into_iter()
        .filter(|(_, bar)| bar.is_object())
        .map(|(label, bar)| bar_text(label, bar))
        .collect();
    let checks: String = data["checks"]
        .as_array()
        .map_or(&[][..], |list| &list[..])
        .iter()
        .map(check_line)
        .collect();
    let checks = if checks.is_empty() {
        checks
    } else {
        format!("checks\n{checks}")
    };
    format!("state: {}\n{bars}{checks}", text(data, "state"))
}

fn change_text(data: &Value) -> String {
    format!(
        "{} {} stream {}, from {:.1}\n",
        data["id"],
        text(data, "state"),
        data["stream_id"],
        number(data, "start_time"),
    )
}

fn clip_text(data: &Value) -> String {
    format!(
        "{} {} {} change {}, {:.1}..{:.1} s\n",
        data["id"],
        text(data, "state"),
        text(data, "kind"),
        data["change_id"],
        number(data, "start"),
        number(data, "end"),
    )
}

fn print_with(json: bool, data: &Value, render: fn(&Value) -> String) -> anyhow::Result<()> {
    if json {
        return print_json(data);
    }
    print!("{}", render(data));
    Ok(())
}

/// The body of `POST /clips`.
fn create_body(change: i64, kind: ClipKind, from: f64, to: f64, version: Option<i64>) -> Value {
    let mut body = Map::new();
    body.insert("change".into(), json!(change));
    body.insert("kind".into(), json!(kind.as_str()));
    body.insert("start".into(), json!(from));
    body.insert("end".into(), json!(to));
    if let Some(version) = version {
        body.insert("version".into(), json!(version));
    }
    Value::Object(body)
}

/// The body of `PATCH /clips/{id}`, with what was given.
fn edit_body(kind: Option<ClipKind>, from: Option<f64>, to: Option<f64>) -> Value {
    let mut body = Map::new();
    if let Some(kind) = kind {
        body.insert("kind".into(), json!(kind.as_str()));
    }
    if let Some(from) = from {
        body.insert("start".into(), json!(from));
    }
    if let Some(to) = to {
        body.insert("end".into(), json!(to));
    }
    Value::Object(body)
}

/// Shows both versions of a change, each from the start of its cycle.
pub(crate) async fn diff(client: &ApiClient, change: i64, json: bool) -> anyhow::Result<()> {
    let data = client.get(&format!("/changes/{change}/diff")).await?;
    print_with(json, &data, comparison_text)
}

/// Closes a change on the operator's word.
pub(crate) async fn dismiss(client: &ApiClient, change: i64, json: bool) -> anyhow::Result<()> {
    let data = client.post(&format!("/changes/{change}/dismiss")).await?;
    print_with(json, &data, change_text)
}

/// Starts a draft clip, shows one, or reshapes a draft.
pub(crate) async fn clip(
    client: &ApiClient,
    action: &ClipAction,
    json: bool,
) -> anyhow::Result<()> {
    let data = match action {
        ClipAction::Create {
            change,
            kind,
            from,
            to,
            version,
        } => {
            let body = create_body(*change, *kind, *from, *to, *version);
            client.post_json("/clips", &body).await?
        }
        ClipAction::Show(clip) => client.get(&format!("/clips/{clip}")).await?,
        ClipAction::Edit {
            clip,
            kind,
            from,
            to,
        } => {
            let body = edit_body(*kind, *from, *to);
            client.patch_json(&format!("/clips/{clip}"), &body).await?
        }
    };
    print_with(json, &data, clip_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comparison_reads_from_the_start_of_each_bar() {
        let data = json!({
            "change_id": 1, "state": "confirmed",
            "old": { "version_id": 3, "start": 12345.0, "period": 3600.0, "regions": [
                { "id": 1, "class": "existing", "start": 12345.0, "end": 13095.0,
                  "video_boundaries": [] },
            ]},
            "new": { "version_id": 4, "start": 20000.0, "period": 3700.0, "regions": [
                { "id": 2, "class": "new", "start": 20000.0, "end": 20100.0,
                  "video_boundaries": [20050.0] },
            ]},
            "checks": [
                { "name": "length", "expected": 455.0, "actual": 454.2, "tolerance": 5.0,
                  "passed": true },
                { "name": "tail", "expected": 1.0, "actual": 9.0, "tolerance": 2.0,
                  "passed": false },
            ],
        });

        assert_eq!(
            comparison_text(&data),
            "state: confirmed\n\
             old  version 3  from 12345.0 s  period 3600.0 s\n\
             \x20 existing  0:00:00\u{2013}0:12:30  (750.0 s)\n\
             new  version 4  from 20000.0 s  period 3700.0 s\n\
             \x20 new  0:00:00\u{2013}0:01:40  (100.0 s)\n\
             \x20   video boundary at 0:00:50\n\
             checks\n\
             \x20 passed  length  expected 455.0  actual 454.2  \u{b1}5.0\n\
             \x20 REVIEW  tail  expected 1.0  actual 9.0  \u{b1}2.0\n"
        );
    }

    #[test]
    fn a_provisional_comparison_shows_only_the_old_bar() {
        let data = json!({
            "state": "provisional", "new": null, "checks": [],
            "old": { "version_id": 3, "start": 0.0, "period": 60.0, "regions": [] },
        });

        assert_eq!(
            comparison_text(&data),
            "state: provisional\nold  version 3  from 0.0 s  period 60.0 s\n"
        );
    }

    #[test]
    fn a_clock_shows_hours_minutes_and_seconds() {
        assert_eq!(clock(0.0), "0:00:00");
        assert_eq!(clock(3_725.0), "1:02:05");
    }

    #[test]
    fn a_new_clip_names_the_stretch() {
        assert_eq!(
            create_body(2, ClipKind::Addition, 1.5, 31.5, None),
            json!({ "change": 2, "kind": "addition", "start": 1.5, "end": 31.5 })
        );
        assert_eq!(
            create_body(2, ClipKind::Cycle, 1.5, 31.5, Some(7))["version"],
            7
        );
    }

    #[test]
    fn an_edit_carries_only_what_was_given() {
        assert_eq!(edit_body(None, Some(4.0), None), json!({ "start": 4.0 }));
        assert_eq!(
            edit_body(Some(ClipKind::Cycle), None, Some(9.0)),
            json!({ "kind": "cycle", "end": 9.0 })
        );
    }
}

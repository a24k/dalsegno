//! Commands that manage streams, programs and known material through the service (FR-C1).

use crate::remote::{PAGE, print_json, print_rows, rows, stream_id, stream_line, text};
use crate::text::probe_text;
use anyhow::anyhow;
use dalsegno_app::probe::ProbeReport;
use dalsegno_app::remote::ApiClient;
use serde_json::{Map, Value, json};
use std::time::Duration;

/// How long `add` waits, since the probe fetches real fragments over each family.
const ADD_WAIT: Duration = Duration::from_secs(5 * 60);
/// The keys of a program that a body carries, as opposed to what the service adds.
const PROGRAM_SERVICE_KEYS: [&str; 3] = ["id", "channel_id", "stream_ids"];

/// What `program` was given.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ProgramArgs {
    pub(crate) short_name: String,
    pub(crate) output_dir: String,
    pub(crate) template: Option<String>,
    pub(crate) suffix: Option<String>,
    pub(crate) artist: Option<String>,
    pub(crate) date: Option<String>,
    pub(crate) description: Option<String>,
}

/// What `known` was asked to do.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum KnownAction {
    List,
    Add {
        name: String,
        stream: String,
        from: f64,
        to: f64,
    },
    Remove(i64),
}

/// The body of `POST` and `PUT /programs`, with the options that were given.
fn program_body(args: &ProgramArgs) -> Value {
    let mut body = Map::new();
    body.insert("short_name".into(), json!(args.short_name));
    body.insert("output_dir".into(), json!(args.output_dir));
    let optional = [
        ("name_template", &args.template),
        ("addition_suffix", &args.suffix),
        ("artist", &args.artist),
        ("content_date", &args.date),
        ("description", &args.description),
    ];
    for (key, value) in optional {
        if let Some(value) = value {
            body.insert(key.into(), json!(value));
        }
    }
    Value::Object(body)
}

fn ids_text(ids: &Value) -> String {
    let ids: Vec<String> = rows(ids).iter().map(Value::to_string).collect();
    if ids.is_empty() {
        "-".to_string()
    } else {
        ids.join(", ")
    }
}

fn program_line(row: &Value) -> String {
    format!(
        "{} {} -> {}, streams {}\n",
        row["id"],
        text(row, "short_name"),
        text(row, "output_dir"),
        ids_text(&row["stream_ids"]),
    )
}

fn value_text(value: &Value) -> String {
    match value {
        Value::Null => "-".to_string(),
        Value::String(text) => text.clone(),
        Value::Array(_) => ids_text(value),
        other => other.to_string(),
    }
}

/// A program as `key: value` lines.
fn program_text(program: &Value) -> String {
    program.as_object().map_or(String::new(), |fields| {
        fields
            .iter()
            .map(|(key, value)| format!("{key}: {}\n", value_text(value)))
            .collect()
    })
}

fn version_line(row: &Value) -> String {
    let until = row["on_air_until"]
        .as_f64()
        .map_or(String::new(), |until| format!(" until {until:.1}"));
    format!(
        "{} stream {}, period {:.1} s, on air from {:.1}{until}\n",
        row["id"],
        row["stream_id"],
        row["period_audio"].as_f64().unwrap_or(0.0),
        row["on_air_from"].as_f64().unwrap_or(0.0),
    )
}

fn known_line(row: &Value) -> String {
    format!(
        "{} {} {:.1} s\n",
        row["id"],
        text(row, "name"),
        row["duration"].as_f64().unwrap_or(0.0),
    )
}

/// Reads `KEY=VALUE` as text, since every field these commands change is text.
fn split_pair(pair: &str) -> anyhow::Result<(&str, &str)> {
    pair.split_once('=')
        .filter(|(key, _)| !key.is_empty())
        .ok_or_else(|| anyhow!("expected KEY=VALUE, got {pair:?}"))
}

/// The body of `PATCH /streams/{id}`.
fn stream_edit_body(pairs: &[String]) -> anyhow::Result<Value> {
    let mut body = Map::new();
    for pair in pairs {
        let (key, value) = split_pair(pair)?;
        body.insert(key.to_string(), json!(value));
    }
    Ok(Value::Object(body))
}

/// The body of `PUT /programs/{id}`: the program as stored, with the given keys replaced.
fn program_edit_body(current: &Value, pairs: &[String]) -> anyhow::Result<Value> {
    let mut body = current.as_object().cloned().unwrap_or_default();
    for key in PROGRAM_SERVICE_KEYS {
        body.remove(key);
    }
    for pair in pairs {
        let (key, value) = split_pair(pair)?;
        body.insert(key.to_string(), json!(value));
    }
    Ok(Value::Object(body))
}

/// The body of `POST /programs/{id}/known-clips`.
fn known_body(name: &str, stream: i64, from: f64, to: f64) -> Value {
    json!({ "name": name, "stream": stream, "start": from, "end": to })
}

/// Registers a stream, and shows it with what the probe found.
pub(crate) async fn add(client: &ApiClient, stream: &str, json: bool) -> anyhow::Result<()> {
    let data = client
        .post_json_waiting("/streams", &json!({ "stream": stream }), ADD_WAIT)
        .await?;
    if json {
        return print_json(&data);
    }
    let probe: ProbeReport = serde_json::from_value(data["probe"].clone())?;
    print!("stream: {}\n{}", data["stream"]["id"], probe_text(&probe));
    Ok(())
}

fn print_program(json: bool, data: &Value) -> anyhow::Result<()> {
    if json {
        print_json(data)
    } else {
        print!("{}", program_text(data));
        Ok(())
    }
}

/// Gives a stream its program: changes the one it has, or makes one and links it.
pub(crate) async fn program(
    client: &ApiClient,
    stream: &str,
    args: &ProgramArgs,
    json: bool,
) -> anyhow::Result<()> {
    let id = stream_id(client, stream).await?;
    let body = program_body(args);
    let row = client.get(&format!("/streams/{id}")).await?;
    let data = match row["program_id"].as_i64() {
        Some(program) => {
            client
                .put_json(&format!("/programs/{program}"), &body)
                .await?
        }
        None => {
            let made = client.post_json("/programs", &body).await?;
            let link = json!({ "program": made["id"] });
            client
                .post_json(&format!("/streams/{id}/link"), &link)
                .await?;
            // The stream is linked now, which the program made does not show yet.
            client.get(&format!("/programs/{}", made["id"])).await?
        }
    };
    if json {
        print_json(&data)
    } else {
        println!("program: {}", data["id"]);
        Ok(())
    }
}

/// Shows the programs.
pub(crate) async fn programs(client: &ApiClient, json: bool) -> anyhow::Result<()> {
    let data = client.get(&format!("/programs{PAGE}")).await?;
    print_rows(json, &data, program_line)
}

/// Changes a stream's title, and shows the stream.
pub(crate) async fn set_stream(
    client: &ApiClient,
    stream: &str,
    pairs: &[String],
    json: bool,
) -> anyhow::Result<()> {
    let id = stream_id(client, stream).await?;
    let data = client
        .patch_json(&format!("/streams/{id}"), &stream_edit_body(pairs)?)
        .await?;
    if json {
        print_json(&data)
    } else {
        print!("{}", stream_line(&data));
        Ok(())
    }
}

/// Changes the given keys of a program, and shows it.
pub(crate) async fn set_program(
    client: &ApiClient,
    program: i64,
    pairs: &[String],
    json: bool,
) -> anyhow::Result<()> {
    let path = format!("/programs/{program}");
    let current = client.get(&path).await?;
    let data = client
        .put_json(&path, &program_edit_body(&current, pairs)?)
        .await?;
    print_program(json, &data)
}

/// Makes a stream part of a program, and shows the stream.
pub(crate) async fn link(
    client: &ApiClient,
    stream: &str,
    program: i64,
    json: bool,
) -> anyhow::Result<()> {
    let id = stream_id(client, stream).await?;
    let data = client
        .post_json(
            &format!("/streams/{id}/link"),
            &json!({ "program": program }),
        )
        .await?;
    if json {
        print_json(&data)
    } else {
        print!("{}", stream_line(&data));
        Ok(())
    }
}

/// Shows a program's versions, newest first.
pub(crate) async fn versions(client: &ApiClient, program: i64, json: bool) -> anyhow::Result<()> {
    let data = client
        .get(&format!("/programs/{program}/versions{PAGE}"))
        .await?;
    print_rows(json, &data, version_line)
}

/// Lists, adds or removes a program's known clips.
pub(crate) async fn known(
    client: &ApiClient,
    program: i64,
    action: &KnownAction,
    json: bool,
) -> anyhow::Result<()> {
    let path = format!("/programs/{program}/known-clips");
    match action {
        KnownAction::List => {
            let data = client.get(&path).await?;
            print_rows(json, &data, known_line)
        }
        KnownAction::Add {
            name,
            stream,
            from,
            to,
        } => {
            let stream = stream_id(client, stream).await?;
            let data = client
                .post_json(&path, &known_body(name, stream, *from, *to))
                .await?;
            if json {
                print_json(&data)
            } else {
                print!("{}", known_line(&data));
                Ok(())
            }
        }
        KnownAction::Remove(clip) => {
            let data = client.delete(&format!("{path}/{clip}")).await?;
            if json {
                print_json(&data)
            } else {
                println!("removed: {}", data["id"]);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> ProgramArgs {
        ProgramArgs {
            short_name: "Example TV".to_string(),
            output_dir: "Example TV".to_string(),
            template: None,
            suffix: None,
            artist: None,
            date: None,
            description: None,
        }
    }

    #[test]
    fn a_program_body_carries_only_the_options_given() {
        let given = ProgramArgs {
            artist: Some("Example".to_string()),
            date: Some("2026-01-01".to_string()),
            ..args()
        };

        assert_eq!(
            program_body(&args()),
            json!({ "short_name": "Example TV", "output_dir": "Example TV" })
        );
        assert_eq!(
            program_body(&given),
            json!({
                "short_name": "Example TV",
                "output_dir": "Example TV",
                "artist": "Example",
                "content_date": "2026-01-01",
            })
        );
    }

    #[test]
    fn a_program_reads_as_one_line() {
        let row = json!({
            "id": 2, "short_name": "Example TV", "output_dir": "Example TV", "stream_ids": [1, 3],
        });
        let alone = json!({
            "id": 3, "short_name": "Other", "output_dir": "Other", "stream_ids": [],
        });

        assert_eq!(
            program_line(&row),
            "2 Example TV -> Example TV, streams 1, 3\n"
        );
        assert_eq!(program_line(&alone), "3 Other -> Other, streams -\n");
    }

    #[test]
    fn a_program_reads_as_key_value_lines() {
        let program =
            json!({ "id": 2, "artist": null, "short_name": "Example TV", "stream_ids": [1] });

        assert_eq!(
            program_text(&program),
            "artist: -\nid: 2\nshort_name: Example TV\nstream_ids: 1\n"
        );
    }

    #[test]
    fn a_version_reads_as_one_line_with_its_end_when_it_has_one() {
        let current = json!({
            "id": 4, "stream_id": 1, "period_audio": 42244.71, "on_air_from": 10.0,
            "on_air_until": null,
        });
        let past = json!({
            "id": 4, "stream_id": 1, "period_audio": 42244.71, "on_air_from": 10.0,
            "on_air_until": 99.95,
        });

        assert_eq!(
            version_line(&current),
            "4 stream 1, period 42244.7 s, on air from 10.0\n"
        );
        assert!(version_line(&past).ends_with("on air from 10.0 until 100.0\n"));
    }

    #[test]
    fn a_known_clip_reads_as_one_line() {
        let row = json!({ "id": 5, "name": "Opening", "duration": 30.04 });

        assert_eq!(known_line(&row), "5 Opening 30.0 s\n");
    }

    #[test]
    fn a_stream_edit_takes_text_as_it_is() {
        assert_eq!(
            stream_edit_body(&["title=2026=new".to_string()]).unwrap(),
            json!({ "title": "2026=new" })
        );
        assert!(stream_edit_body(&["title".to_string()]).is_err());
        assert!(stream_edit_body(&["=x".to_string()]).is_err());
    }

    #[test]
    fn a_program_edit_replaces_the_given_keys_and_drops_what_the_service_adds() {
        let current = json!({
            "id": 2, "channel_id": "UC", "stream_ids": [1], "short_name": "Example TV",
            "output_dir": "Example TV", "artist": null,
        });

        let body = program_edit_body(
            &current,
            &["artist=Example".to_string(), "date=1".to_string()],
        )
        .unwrap();

        assert_eq!(
            body,
            json!({
                "short_name": "Example TV", "output_dir": "Example TV",
                "artist": "Example", "date": "1",
            })
        );
    }

    #[test]
    fn a_known_clip_body_names_the_stretch() {
        assert_eq!(
            known_body("Opening", 4, 1.5, 31.5),
            json!({ "name": "Opening", "stream": 4, "start": 1.5, "end": 31.5 })
        );
    }
}

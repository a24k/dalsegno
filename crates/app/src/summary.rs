//! The opt-in daily summary of every stream, sent through the outbox like the others (D-25).
//!
//! The time of day is in Japan, where the operator is, as notifications show it.

use crate::notify::SUMMARY;
use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone};
use chrono_tz::Asia::Tokyo;
use dalsegno_store::database::{Database, StoreError};
use dalsegno_store::streams::StreamState;
use serde_json::{Value, json};

const DAY_MS: i64 = 24 * 3_600_000;

fn date_in_japan(ms: i64) -> Option<NaiveDate> {
    Some(
        DateTime::from_timestamp_millis(ms)?
            .with_timezone(&Tokyo)
            .date_naive(),
    )
}

/// The summary that is due at `now`, as its date and the time its failures are counted from.
/// One is due each day in Japan once the time of day has passed, so moving the time later
/// does not send a second one that day.
fn due(at: NaiveTime, last: Option<i64>, now: i64) -> Option<(String, i64)> {
    let today = date_in_japan(now)?;
    let scheduled = Tokyo
        .from_local_datetime(&today.and_time(at))
        .single()?
        .timestamp_millis();
    let is_sent_today = last.and_then(date_in_japan) == Some(today);
    let is_due = now >= scheduled && !is_sent_today;
    is_due.then(|| (today.to_string(), last.unwrap_or(now - DAY_MS)))
}

/// What every stream that has not ended looks like now, and the jobs that failed since.
fn snapshot(database: &Database, since: i64) -> Result<Value, StoreError> {
    let streams: Vec<Value> = database
        .list_streams(u32::MAX, 0)?
        .into_iter()
        .filter(|stream| stream.state != StreamState::Ended)
        .map(|stream| {
            json!({
                "stream": stream.title,
                "state": stream.state.as_str(),
                "period": stream.period,
                "open": stream.open_fragments,
                "lost": stream.lost_fragments,
            })
        })
        .collect();
    let failed: Vec<Value> = database
        .failed_jobs_since(since)?
        .into_iter()
        .map(|(kind, count)| json!({ "kind": kind, "count": count }))
        .collect();
    Ok(json!({ "since": since, "streams": streams, "failed": failed }))
}

/// Queues the summary if one is due at `at` each day, and returns whether it did.
pub fn enqueue_summary_if_due(
    database: &Database,
    at: NaiveTime,
    now: i64,
) -> Result<bool, StoreError> {
    let Some((date, since)) = due(at, database.last_notification_at(SUMMARY)?, now) else {
        return Ok(false);
    };
    let body = snapshot(database, since)?;
    let subject = format!("summary:{date}");
    database.enqueue_notification(SUMMARY, &subject, &body.to_string(), now)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dalsegno_store::jobs::{JobClass, NewJob};
    use dalsegno_store::streams::NewStream;

    /// 11/04 09:00 in Japan.
    const NINE: i64 = 1_793_750_400_000;
    const MINUTE_MS: i64 = 60_000;

    fn nine() -> NaiveTime {
        NaiveTime::from_hms_opt(9, 0, 0).unwrap()
    }

    fn summaries(database: &Database) -> Vec<(String, Value)> {
        database
            .pending_notifications(100)
            .unwrap()
            .into_iter()
            .filter(|notice| notice.kind == SUMMARY)
            .map(|notice| (notice.subject, serde_json::from_str(&notice.body).unwrap()))
            .collect()
    }

    fn with_stream(state: StreamState) -> Database {
        let database = Database::open_in_memory().unwrap();
        let id = database
            .insert_stream(&NewStream {
                video_id: "abcDEF123_-".to_string(),
                title: "Example TV".to_string(),
                channel_id: None,
                channel_name: None,
                thumbnail_url: None,
                video_itag: None,
                audio_itag: 140,
                fragment_len: 5.0,
                registered_at: 0,
            })
            .unwrap();
        database.set_stream_state(id, state, None).unwrap();
        database
    }

    #[test]
    fn the_test_time_is_nine_in_japan() {
        let at = DateTime::parse_from_rfc3339("2026-11-04T09:00:00+09:00").unwrap();

        assert_eq!(at.timestamp_millis(), NINE);
    }

    #[test]
    fn nothing_is_due_before_the_time() {
        assert_eq!(due(nine(), None, NINE - MINUTE_MS), None);
    }

    #[test]
    fn the_first_summary_counts_the_last_day() {
        assert_eq!(
            due(nine(), None, NINE),
            Some(("2026-11-04".to_string(), NINE - DAY_MS))
        );
    }

    #[test]
    fn a_summary_is_sent_once_a_day() {
        let sent = NINE + MINUTE_MS;

        assert_eq!(due(nine(), Some(sent), NINE + 2 * MINUTE_MS), None);
        assert_eq!(
            due(nine(), Some(sent), NINE + DAY_MS),
            Some(("2026-11-05".to_string(), sent))
        );
    }

    #[test]
    fn a_later_time_on_the_same_day_sends_nothing_more() {
        let sent = NINE + MINUTE_MS;
        let noon = NaiveTime::from_hms_opt(12, 0, 0).unwrap();

        assert_eq!(due(noon, Some(sent), NINE + 3 * 60 * MINUTE_MS), None);
    }

    #[test]
    fn a_late_start_still_sends_that_day() {
        let yesterday = NINE - DAY_MS;

        assert_eq!(
            due(nine(), Some(yesterday), NINE + 5 * 60 * MINUTE_MS),
            Some(("2026-11-04".to_string(), yesterday))
        );
    }

    #[test]
    fn the_summary_holds_each_stream_and_the_failures_since() {
        let database = with_stream(StreamState::Recording);
        let job = NewJob {
            kind: "backfill".to_string(),
            class: JobClass::Cpu,
            target: "{}".to_string(),
        };
        database.enqueue_job(&job, 0).unwrap();
        let claimed = database.claim_job(JobClass::Cpu, 0).unwrap().unwrap();
        database
            .finish_job(claimed.id, Err("offline"), NINE - MINUTE_MS)
            .unwrap();

        assert!(enqueue_summary_if_due(&database, nine(), NINE).unwrap());

        let (subject, body) = &summaries(&database)[0];
        assert_eq!(subject, "summary:2026-11-04");
        assert_eq!(
            body["streams"],
            json!([{
                "stream": "Example TV",
                "state": "recording",
                "period": null,
                "open": 0,
                "lost": 0,
            }])
        );
        assert_eq!(body["failed"], json!([{ "kind": "backfill", "count": 1 }]));
        assert_eq!(body["since"], NINE - DAY_MS);
    }

    #[test]
    fn an_ended_stream_is_left_out_and_a_second_look_sends_nothing() {
        let database = with_stream(StreamState::Ended);

        assert!(enqueue_summary_if_due(&database, nine(), NINE).unwrap());
        assert!(!enqueue_summary_if_due(&database, nine(), NINE + MINUTE_MS).unwrap());

        let sent = summaries(&database);
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].1["streams"], json!([]));
    }
}

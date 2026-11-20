//! Operational settings (D-24): the values the file, environment and flags start from, with
//! the overrides the settings screen stores. A change applies at once.

use crate::config::parse_summary_time;
use crate::fetch::{FetchLimits, SharedDatabase, lock};
use chrono::NaiveTime;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use tokio::sync::watch;

/// Requests in flight.
pub const CONCURRENCY: &str = "concurrency";
/// Request starts per second.
pub const REQUESTS_PER_SECOND: &str = "requests_per_second";
/// When the daily summary goes out, in Japan time, or `off`.
pub const DAILY_SUMMARY: &str = "daily_summary";
/// The keys `PUT /settings` takes.
pub const OPERATIONAL: [&str; 3] = [CONCURRENCY, REQUESTS_PER_SECOND, DAILY_SUMMARY];
/// Keys that only the file and environment set, as paths, addresses and secrets.
pub const BOOTSTRAP: [&str; 9] = [
    "data",
    "library",
    "bind",
    "cookies",
    "webhook",
    "webhook_format",
    "public_url",
    "plex_url",
    "plex_token",
];
/// More requests in flight than this would only be refused by the origin.
const MAX_CONCURRENCY: u64 = 64;
const OFF: &str = "off";
const TIME_FORMAT: &str = "%H:%M";

/// Why a change of settings was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingsError {
    /// The key is set only by the file and environment.
    #[error("{0} is set in the file or environment, not here")]
    Bootstrap(String),
    /// No such setting.
    #[error("there is no setting {0}")]
    Unknown(String),
    /// The value is out of range or of the wrong kind.
    #[error("{key}: {reason}")]
    Invalid {
        key: &'static str,
        reason: &'static str,
    },
    /// The database failed.
    #[error("the settings could not be stored: {0}")]
    Store(String),
}

/// The values the service runs with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Operational {
    /// The request budget every stream and job shares.
    pub limits: FetchLimits,
    /// When the daily summary goes out. None turns it off.
    pub daily_summary: Option<NaiveTime>,
}

impl Operational {
    fn value(&self, key: &str) -> Value {
        match key {
            CONCURRENCY => json!(self.limits.concurrency),
            REQUESTS_PER_SECOND => json!(self.limits.requests_per_second),
            _ => self
                .daily_summary
                .map_or(json!(OFF), |at| json!(at.format(TIME_FORMAT).to_string())),
        }
    }

    /// Applies one value of an operational key.
    fn with(self, key: &'static str, value: &Value) -> Result<Self, SettingsError> {
        let invalid = |reason| SettingsError::Invalid { key, reason };
        let next = match key {
            CONCURRENCY => {
                let count = value
                    .as_u64()
                    .filter(|count| (1..=MAX_CONCURRENCY).contains(count))
                    .ok_or(invalid("a whole number from 1 to 64"))?;
                let limits = FetchLimits {
                    concurrency: count as usize,
                    ..self.limits
                };
                Self { limits, ..self }
            }
            REQUESTS_PER_SECOND => {
                let rate = value.as_f64().ok_or(invalid("a number"))?;
                let limits = FetchLimits {
                    requests_per_second: rate,
                    ..self.limits
                };
                limits.validate().map_err(|_| invalid("out of range"))?;
                Self { limits, ..self }
            }
            _ => {
                let text = value
                    .as_str()
                    .ok_or(invalid("a time such as 09:00, or off"))?;
                let daily_summary = if text == OFF {
                    None
                } else {
                    Some(parse_summary_time(text).ok_or(invalid("a time such as 09:00, or off"))?)
                };
                Self {
                    daily_summary,
                    ..self
                }
            }
        };
        Ok(next)
    }
}

fn operational_key(key: &str) -> Result<&'static str, SettingsError> {
    if let Some(known) = OPERATIONAL.iter().find(|known| **known == key) {
        return Ok(known);
    }
    if BOOTSTRAP.contains(&key) {
        return Err(SettingsError::Bootstrap(key.to_string()));
    }
    Err(SettingsError::Unknown(key.to_string()))
}

/// Reads the stored overrides over `base`. One that no longer reads, which only an older or
/// newer build could have stored, is left off and named.
pub fn with_overrides(
    base: Operational,
    stored: &[(String, String)],
) -> (Operational, Vec<String>) {
    stored
        .iter()
        .fold((base, Vec::new()), |(current, mut ignored), (key, text)| {
            let value: Option<Value> = serde_json::from_str(text).ok();
            let next = operational_key(key)
                .ok()
                .zip(value)
                .and_then(|(key, value)| current.with(key, &value).ok());
            match next {
                Some(next) => (next, ignored),
                None => {
                    ignored.push(key.clone());
                    (current, ignored)
                }
            }
        })
}

/// The settings in force, which every part of the service reads.
#[derive(Debug, Clone)]
pub struct LiveSettings {
    base: Operational,
    database: SharedDatabase,
    current: Arc<watch::Sender<Operational>>,
}

impl LiveSettings {
    /// Starts from `base` with what the database overrides, and names overrides left off.
    pub fn load(
        base: Operational,
        database: SharedDatabase,
    ) -> Result<(Self, Vec<String>), SettingsError> {
        let stored = lock(&database)
            .map_err(|error| SettingsError::Store(error.to_string()))?
            .settings()
            .map_err(|error| SettingsError::Store(error.to_string()))?;
        let (current, ignored) = with_overrides(base, &stored);
        let (sender, _) = watch::channel(current);
        let settings = Self {
            base,
            database,
            current: Arc::new(sender),
        };
        Ok((settings, ignored))
    }

    /// The values in force.
    pub fn current(&self) -> Operational {
        *self.current.borrow()
    }

    /// The values in force from now on, as they change.
    pub fn subscribe(&self) -> watch::Receiver<Operational> {
        self.current.subscribe()
    }

    /// Each operational setting with the value in force, the value from the file,
    /// environment or flags it falls back to, and whether the two differ.
    pub fn view(&self) -> Value {
        let current = self.current();
        let entries = OPERATIONAL.iter().map(|key| {
            let value = current.value(key);
            let default = self.base.value(key);
            let is_overridden = value != default;
            (
                key.to_string(),
                json!({ "value": value, "default": default, "overridden": is_overridden }),
            )
        });
        Value::Object(entries.collect())
    }

    /// Applies a change: each key to a value, or to null to drop its override. Every key is
    /// checked before anything is stored, so a refused change leaves everything as it was.
    pub fn update(&self, change: &Map<String, Value>) -> Result<Operational, SettingsError> {
        let parsed = change
            .iter()
            .map(|(key, value)| operational_key(key).map(|key| (key, value)))
            .collect::<Result<Vec<_>, _>>()?;
        let next = parsed
            .iter()
            .try_fold(self.current(), |current, (key, value)| {
                if value.is_null() {
                    current.with(key, &self.base.value(key))
                } else {
                    current.with(key, value)
                }
            })?;
        self.store(&parsed)?;
        self.current.send_replace(next);
        Ok(next)
    }

    fn store(&self, parsed: &[(&'static str, &Value)]) -> Result<(), SettingsError> {
        let failed = |error: String| SettingsError::Store(error);
        let database = lock(&self.database).map_err(|error| failed(error.to_string()))?;
        parsed.iter().try_for_each(|(key, value)| {
            let done = if value.is_null() {
                database.delete_setting(key)
            } else {
                database.put_setting(key, &value.to_string())
            };
            done.map_err(|error| failed(error.to_string()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acquisition::Library;
    use tempfile::tempdir;

    fn base() -> Operational {
        Operational {
            limits: FetchLimits::default(),
            daily_summary: NaiveTime::from_hms_opt(9, 0, 0),
        }
    }

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    fn live() -> (tempfile::TempDir, LiveSettings) {
        let root = tempdir().unwrap();
        let database = Library::open(root.path()).unwrap().database();
        let (settings, ignored) = LiveSettings::load(base(), database).unwrap();
        assert!(ignored.is_empty());
        (root, settings)
    }

    #[test]
    fn a_change_applies_at_once_and_is_seen_by_subscribers() {
        let (_root, settings) = live();
        let mut updates = settings.subscribe();

        let next = settings
            .update(&object(json!({ "concurrency": 2, "daily_summary": "off" })))
            .unwrap();

        assert_eq!(next.limits.concurrency, 2);
        assert_eq!(next.daily_summary, None);
        assert!(updates.has_changed().unwrap());
        assert_eq!(*updates.borrow_and_update(), next);
    }

    #[test]
    fn overrides_survive_a_restart_and_null_drops_one() {
        let (_root, settings) = live();
        settings
            .update(&object(
                json!({ "requests_per_second": 1.5, "concurrency": 3 }),
            ))
            .unwrap();
        settings
            .update(&object(json!({ "concurrency": null })))
            .unwrap();

        let (again, ignored) = LiveSettings::load(base(), settings.database.clone()).unwrap();

        assert!(ignored.is_empty());
        assert_eq!(again.current().limits.requests_per_second, 1.5);
        assert_eq!(
            again.current().limits.concurrency,
            base().limits.concurrency
        );
    }

    #[test]
    fn the_view_shows_the_value_the_default_and_whether_it_is_overridden() {
        let (_root, settings) = live();
        settings
            .update(&object(json!({ "daily_summary": "21:30" })))
            .unwrap();

        let view = settings.view();

        assert_eq!(
            view["daily_summary"],
            json!({ "value": "21:30", "default": "09:00", "overridden": true })
        );
        assert_eq!(view["concurrency"]["overridden"], false);
    }

    #[test]
    fn a_bootstrap_or_unknown_key_is_refused_and_nothing_changes() {
        let (_root, settings) = live();
        let before = settings.current();

        let bootstrap = settings.update(&object(json!({ "concurrency": 2, "webhook": "x" })));
        let unknown = settings.update(&object(json!({ "colour": "red" })));

        assert_eq!(
            bootstrap,
            Err(SettingsError::Bootstrap("webhook".to_string()))
        );
        assert_eq!(unknown, Err(SettingsError::Unknown("colour".to_string())));
        assert_eq!(settings.current(), before);
    }

    #[test]
    fn values_out_of_range_are_refused() {
        let (_root, settings) = live();

        for change in [
            json!({ "concurrency": 0 }),
            json!({ "concurrency": 65 }),
            json!({ "concurrency": 1.5 }),
            json!({ "requests_per_second": 0 }),
            json!({ "requests_per_second": "fast" }),
            json!({ "daily_summary": "25:00" }),
        ] {
            let refused = settings.update(&object(change.clone()));
            assert!(
                matches!(refused, Err(SettingsError::Invalid { .. })),
                "{change} was taken"
            );
        }
        assert_eq!(settings.current(), base());
    }

    #[test]
    fn a_stored_override_that_does_not_read_is_left_off_and_named() {
        let stored = [
            ("concurrency".to_string(), "\"many\"".to_string()),
            ("requests_per_second".to_string(), "2".to_string()),
            ("retired".to_string(), "1".to_string()),
        ];

        let (current, ignored) = with_overrides(base(), &stored);

        assert_eq!(current.limits.concurrency, base().limits.concurrency);
        assert_eq!(current.limits.requests_per_second, 2.0);
        assert_eq!(ignored, ["concurrency", "retired"]);
    }
}

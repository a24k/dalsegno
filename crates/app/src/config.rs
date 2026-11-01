//! Bootstrap settings, from a TOML file and `DALSEGNO_*` environment variables (D-24).
//!
//! The environment wins, so a container can override a mounted file.

use dalsegno_core::redact::Redacted;
use serde::Deserialize;
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Loopback, as the service sits behind an authenticating proxy (D-20).
const DEFAULT_BIND: &str = "127.0.0.1:8737";
const CONFIG_ENV: &str = "DALSEGNO_CONFIG";
const DEFAULT_FILE: &str = ".config/dalsegno/config.toml";

/// Why settings could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The file exists and cannot be read.
    #[error("cannot read the settings file {path}: {source}")]
    Read {
        /// The file.
        path: PathBuf,
        /// What went wrong.
        source: std::io::Error,
    },
    /// The file is not valid. The parser's message is left out, as it quotes the line.
    #[error("the settings file {0} is not valid")]
    Parse(PathBuf),
    /// A required setting is missing.
    #[error("{0} is not set")]
    Missing(&'static str),
    /// A setting has the wrong form.
    #[error("{0} is not valid")]
    Invalid(&'static str),
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileSettings {
    data: Option<PathBuf>,
    library: Option<PathBuf>,
    bind: Option<String>,
    cookies: Option<PathBuf>,
    webhook: Option<String>,
    public_url: Option<String>,
    plex_url: Option<String>,
    plex_token: Option<String>,
}

/// The settings the service starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Fragments, database and work area.
    pub data: PathBuf,
    /// Library root.
    pub library: Option<PathBuf>,
    /// Address of the API.
    pub bind: SocketAddr,
    /// Cookie file for yt-dlp.
    pub cookies: Option<PathBuf>,
    /// Notification webhook, which can embed a secret.
    pub webhook: Option<Redacted<String>>,
    /// Base of the links in notifications.
    pub public_url: Option<String>,
    /// Plex base URL.
    pub plex_url: Option<String>,
    /// Plex token.
    pub plex_token: Option<Redacted<String>>,
}

fn read_file(path: &Path, is_required: bool) -> Result<FileSettings, ConfigError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound && !is_required => {
            return Ok(FileSettings::default());
        }
        Err(source) => {
            let path = path.to_path_buf();
            return Err(ConfigError::Read { path, source });
        }
    };
    toml::from_str(&text).map_err(|_| ConfigError::Parse(path.to_path_buf()))
}

impl Config {
    /// Reads the file, if there is one, and the environment of this process. A file named by
    /// `DALSEGNO_CONFIG` must exist.
    pub fn load() -> Result<Self, ConfigError> {
        let read = |name: &str| env::var(name).ok().filter(|value| !value.is_empty());
        let named = read(CONFIG_ENV).map(PathBuf::from);
        let is_required = named.is_some();
        let path = named.or_else(|| read("HOME").map(|home| Path::new(&home).join(DEFAULT_FILE)));
        let file = match path {
            Some(path) => read_file(&path, is_required)?,
            None => FileSettings::default(),
        };
        Self::merge(file, read)
    }

    fn merge(
        file: FileSettings,
        read: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, ConfigError> {
        let data = read("DALSEGNO_DATA").map(PathBuf::from).or(file.data);
        let bind = read("DALSEGNO_BIND")
            .or(file.bind)
            .unwrap_or_else(|| DEFAULT_BIND.to_string());
        Ok(Self {
            data: data.ok_or(ConfigError::Missing("DALSEGNO_DATA"))?,
            library: read("DALSEGNO_LIBRARY").map(PathBuf::from).or(file.library),
            bind: bind
                .parse()
                .map_err(|_| ConfigError::Invalid("DALSEGNO_BIND"))?,
            cookies: read("DALSEGNO_COOKIES").map(PathBuf::from).or(file.cookies),
            webhook: read("DALSEGNO_WEBHOOK").or(file.webhook).map(Redacted::new),
            public_url: read("DALSEGNO_PUBLIC_URL").or(file.public_url),
            plex_url: read("DALSEGNO_PLEX_URL").or(file.plex_url),
            plex_token: read("DALSEGNO_PLEX_TOKEN")
                .or(file.plex_token)
                .map(Redacted::new),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn no_environment(_: &str) -> Option<String> {
        None
    }

    fn file(text: &str) -> FileSettings {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn the_file_gives_the_settings() {
        let config = Config::merge(
            file("data = \"/data\"\nlibrary = \"/library\"\nbind = \"0.0.0.0:9000\""),
            no_environment,
        )
        .unwrap();

        assert_eq!(config.data, PathBuf::from("/data"));
        assert_eq!(config.library, Some(PathBuf::from("/library")));
        assert_eq!(config.bind.port(), 9000);
    }

    #[test]
    fn the_service_stays_on_loopback_unless_told() {
        let config = Config::merge(file("data = \"/data\""), no_environment).unwrap();

        assert!(config.bind.ip().is_loopback());
    }

    #[test]
    fn the_environment_wins() {
        let environment = |name: &str| (name == "DALSEGNO_DATA").then(|| "/other".to_string());

        let config = Config::merge(file("data = \"/data\""), environment).unwrap();

        assert_eq!(config.data, PathBuf::from("/other"));
    }

    #[test]
    fn the_data_directory_is_required() {
        let error = Config::merge(FileSettings::default(), no_environment).unwrap_err();

        assert!(matches!(error, ConfigError::Missing("DALSEGNO_DATA")));
    }

    #[test]
    fn a_bad_address_is_refused() {
        let error =
            Config::merge(file("data = \"/d\"\nbind = \"nowhere\""), no_environment).unwrap_err();

        assert!(matches!(error, ConfigError::Invalid("DALSEGNO_BIND")));
    }

    #[test]
    fn secrets_stay_out_of_debug_output() {
        let config = Config::merge(
            file("data = \"/d\"\nplex_token = \"sesame\"\nwebhook = \"https://x.invalid/sesame\""),
            no_environment,
        )
        .unwrap();

        assert!(!format!("{config:?}").contains("sesame"));
    }

    #[test]
    fn an_unknown_key_is_refused() {
        assert!(toml::from_str::<FileSettings>("dat = \"/d\"").is_err());
    }

    #[test]
    fn a_default_file_may_be_missing() {
        let root = tempdir().unwrap();

        assert!(read_file(&root.path().join("none.toml"), false).is_ok());
    }

    #[test]
    fn a_named_file_must_exist() {
        let root = tempdir().unwrap();

        let error = read_file(&root.path().join("none.toml"), true).unwrap_err();

        assert!(matches!(error, ConfigError::Read { .. }));
    }

    #[test]
    fn a_broken_file_does_not_echo_its_contents() {
        let root = tempdir().unwrap();
        let path = root.path().join("config.toml");
        fs::write(&path, "plex_token = sesame").unwrap();

        let error = read_file(&path, false).unwrap_err();

        assert!(!error.to_string().contains("sesame"));
    }
}

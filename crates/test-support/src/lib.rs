//! Test helpers shared across the workspace. Use as a dev-dependency only.

use std::env;
use std::path::PathBuf;

/// Name of the environment variable that points at the fixtures directory.
pub const FIXTURES_ENV: &str = "DALSEGNO_FIXTURES";

/// Returns the fixtures directory, or `None` when the variable is unset or
/// does not name an existing directory.
pub fn fixtures_dir() -> Option<PathBuf> {
    fixtures_dir_from(env::var_os(FIXTURES_ENV))
}

fn fixtures_dir_from(value: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(value?);
    path.is_dir().then_some(path)
}

/// Returns early from the enclosing test, printing a skip message, when the
/// fixtures directory is not available.
#[macro_export]
macro_rules! require_fixtures {
    () => {
        match $crate::fixtures_dir() {
            Some(dir) => dir,
            None => {
                eprintln!(
                    "skipping: {} is unset or not a directory",
                    $crate::FIXTURES_ENV
                );
                return;
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn returns_none_when_unset() {
        assert_eq!(fixtures_dir_from(None), None);
    }

    #[test]
    fn returns_none_when_not_a_directory() {
        let missing = OsString::from("/definitely/not/a/real/dir");
        assert_eq!(fixtures_dir_from(Some(missing)), None);
    }

    #[test]
    fn returns_path_when_directory() {
        let dir = env::temp_dir();
        assert_eq!(fixtures_dir_from(Some(dir.clone().into())), Some(dir));
    }

    #[test]
    fn macro_skips_or_yields_directory() {
        let dir = require_fixtures!();
        assert!(dir.is_dir());
    }
}

//! Names of exported items (FR-E7), which become folder and file names.

use std::fmt::Write;
use thiserror::Error;
use unicode_normalization::UnicodeNormalization;

/// The template a program starts with.
pub const DEFAULT_TEMPLATE: &str = "{name} {datetime}{suffix} ({year})";
/// The addition suffix a program starts with.
pub const DEFAULT_SUFFIX: &str = "追加分";

const JST_OFFSET_SECONDS: i64 = 9 * 3600;
const SECONDS_PER_DAY: i64 = 86_400;
/// Longest name, in bytes. File systems allow 255, and suffixes such as `-fanart.jpg` add to it.
const MAX_NAME_BYTES: usize = 200;

/// What an export holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    /// The first cycle of a program, named by the date it was acquired.
    FirstCycle,
    /// A later cycle, named by the moment the playlist switched.
    Cycle,
    /// Only what a version added, named by when it began in the observed cycle.
    Addition,
}

/// Why a name cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NameError {
    /// The template uses a placeholder that does not exist.
    #[error("unknown placeholder {{{0}}} in the name template")]
    UnknownPlaceholder(String),
    /// A brace in the template is not closed.
    #[error("unclosed placeholder in the name template")]
    UnclosedPlaceholder,
    /// Nothing is left.
    #[error("the name is empty")]
    Empty,
    /// The name is too long for a file system.
    #[error("the name is {0} bytes, and at most {MAX_NAME_BYTES} fit")]
    TooLong(usize),
    /// The name would leave its folder, or hide from Plex.
    #[error("the name starts with a dot")]
    LeadingDot,
    /// The name holds a path separator or a control character.
    #[error("the name holds the forbidden character {0:?}")]
    ForbiddenCharacter(char),
    /// Some file systems strip these, which changes the name.
    #[error("the name ends with a space or a dot")]
    BadEnding,
}

/// A time in JST, from which names are built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JstTime {
    /// Year.
    pub year: i64,
    /// Month, from 1.
    pub month: u32,
    /// Day of the month, from 1.
    pub day: u32,
    /// Hour.
    pub hour: u32,
    /// Minute.
    pub minute: u32,
}

impl JstTime {
    /// Converts Unix seconds.
    pub fn from_unix(seconds: i64) -> Self {
        let local = seconds + JST_OFFSET_SECONDS;
        let (year, month, day) = civil_from_days(local.div_euclid(SECONDS_PER_DAY));
        let second_of_day = local.rem_euclid(SECONDS_PER_DAY);
        Self {
            year,
            month,
            day,
            hour: (second_of_day / 3600) as u32,
            minute: (second_of_day / 60 % 60) as u32,
        }
    }

    fn date(self) -> String {
        format!("{:04}{:02}{:02}", self.year, self.month, self.day)
    }

    fn minute_stamp(self) -> String {
        format!("{}-{:02}{:02}", self.date(), self.hour, self.minute)
    }
}

/// Turns days since 1970-01-01 into a date, by Howard Hinnant's `civil_from_days`.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    const DAYS_PER_ERA: i64 = 146_097;
    let shifted = days + 719_468;
    let era = shifted.div_euclid(DAYS_PER_ERA);
    let day_of_era = shifted.rem_euclid(DAYS_PER_ERA);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// What a name is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameParts<'a> {
    /// Program template.
    pub template: &'a str,
    /// Program short name.
    pub name: &'a str,
    /// Program addition suffix.
    pub suffix: &'a str,
    /// What the export holds.
    pub kind: ExportKind,
    /// The moment the name refers to, in Unix seconds.
    pub at: i64,
}

/// A checked name, with the title that goes with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportName {
    /// Folder and file name, without an extension.
    pub name: String,
    /// The `title` tag: the same name without the year, which Plex reads from the file name.
    pub title: String,
}

/// Renders and checks a name.
pub fn render_name(parts: &NameParts<'_>) -> Result<ExportName, NameError> {
    let time = JstTime::from_unix(parts.at);
    let datetime = match parts.kind {
        ExportKind::FirstCycle => time.date(),
        ExportKind::Cycle | ExportKind::Addition => time.minute_stamp(),
    };
    let suffix = match parts.kind {
        ExportKind::Addition => format!(" {}", parts.suffix),
        ExportKind::FirstCycle | ExportKind::Cycle => String::new(),
    };
    let rendered = expand(parts.template, parts.name, &datetime, &suffix, time.year)?;
    let name = checked(&rendered)?;
    let title = name
        .strip_suffix(&format!(" ({})", time.year))
        .unwrap_or(&name)
        .to_string();
    Ok(ExportName { name, title })
}

fn expand(
    template: &str,
    name: &str,
    datetime: &str,
    suffix: &str,
    year: i64,
) -> Result<String, NameError> {
    let mut output = String::new();
    let mut rest = template;
    while let Some((literal, after)) = rest.split_once('{') {
        output.push_str(literal);
        let (key, tail) = after
            .split_once('}')
            .ok_or(NameError::UnclosedPlaceholder)?;
        match key {
            "name" => output.push_str(name),
            "datetime" => output.push_str(datetime),
            "suffix" => output.push_str(suffix),
            "year" => {
                let _ = write!(output, "{year}");
            }
            other => return Err(NameError::UnknownPlaceholder(other.to_string())),
        }
        rest = tail;
    }
    output.push_str(rest);
    Ok(output)
}

/// Normalises a name, and rejects whatever could escape a folder or break a file system.
fn checked(name: &str) -> Result<String, NameError> {
    let normal: String = name.nfc().collect();
    if normal.is_empty() {
        return Err(NameError::Empty);
    }
    if normal.len() > MAX_NAME_BYTES {
        return Err(NameError::TooLong(normal.len()));
    }
    if normal.starts_with('.') {
        return Err(NameError::LeadingDot);
    }
    if let Some(bad) = normal
        .chars()
        .find(|c| matches!(c, '/' | '\\' | ':') || c.is_control())
    {
        return Err(NameError::ForbiddenCharacter(bad));
    }
    if normal.ends_with([' ', '.']) {
        return Err(NameError::BadEnding);
    }
    Ok(normal)
}

/// Checks a program's output folder, which sits inside the library and must stay there.
pub fn check_output_dir(folder: &str) -> Result<(), NameError> {
    if folder.starts_with('/') {
        return Err(NameError::ForbiddenCharacter('/'));
    }
    folder
        .split('/')
        .try_for_each(|part| checked(part).map(|_| ()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2026-10-30 23:56 JST.
    const ADDITION_AT: i64 = 1_793_372_160;

    fn parts(kind: ExportKind) -> NameParts<'static> {
        NameParts {
            template: DEFAULT_TEMPLATE,
            name: "Example TV",
            suffix: DEFAULT_SUFFIX,
            kind,
            at: ADDITION_AT,
        }
    }

    fn with_name(name: &str) -> Result<ExportName, NameError> {
        render_name(&NameParts {
            name,
            ..parts(ExportKind::Cycle)
        })
    }

    #[test]
    fn each_kind_has_its_own_name() {
        let name = |kind| render_name(&parts(kind)).unwrap().name;

        assert_eq!(name(ExportKind::FirstCycle), "Example TV 20261030 (2026)");
        assert_eq!(name(ExportKind::Cycle), "Example TV 20261030-2356 (2026)");
        assert_eq!(
            name(ExportKind::Addition),
            "Example TV 20261030-2356 追加分 (2026)"
        );
    }

    #[test]
    fn the_title_drops_the_year() {
        let name = render_name(&parts(ExportKind::Addition)).unwrap();

        assert_eq!(name.title, "Example TV 20261030-2356 追加分");
    }

    #[test]
    fn times_are_in_jst_across_a_year_end() {
        // 2026-12-31 15:30 UTC is 2027-01-01 00:30 in JST.
        let time = JstTime::from_unix(1_798_731_000);

        assert_eq!(
            time,
            JstTime {
                year: 2027,
                month: 1,
                day: 1,
                hour: 0,
                minute: 30
            }
        );
    }

    #[test]
    fn the_year_is_the_jst_year() {
        let name = render_name(&NameParts {
            at: 1_798_731_000,
            ..parts(ExportKind::Cycle)
        })
        .unwrap();

        assert_eq!(name.name, "Example TV 20270101-0030 (2027)");
    }

    #[test]
    fn a_leap_day_is_a_date() {
        // 2028-02-29 12:00 UTC.
        let time = JstTime::from_unix(1_835_438_400);

        assert_eq!((time.year, time.month, time.day), (2028, 2, 29));
    }

    #[test]
    fn times_before_1970_work() {
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn a_template_can_reorder_its_parts() {
        let name = render_name(&NameParts {
            template: "{year} - {name}",
            ..parts(ExportKind::Cycle)
        })
        .unwrap();

        assert_eq!(name.name, "2026 - Example TV");
    }

    #[test]
    fn unknown_placeholders_are_rejected() {
        let result = render_name(&NameParts {
            template: "{name} {oops}",
            ..parts(ExportKind::Cycle)
        });

        assert_eq!(
            result,
            Err(NameError::UnknownPlaceholder("oops".to_string()))
        );
    }

    #[test]
    fn unclosed_placeholders_are_rejected() {
        let result = render_name(&NameParts {
            template: "{name",
            ..parts(ExportKind::Cycle)
        });

        assert_eq!(result, Err(NameError::UnclosedPlaceholder));
    }

    #[test]
    fn names_that_escape_a_folder_are_rejected() {
        for bad in ["a/b", "a\\b", "a:b", "../../etc/passwd"] {
            assert!(
                matches!(
                    with_name(bad),
                    Err(NameError::ForbiddenCharacter(_) | NameError::LeadingDot)
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn control_characters_are_rejected() {
        for bad in ["a\nb", "a\tb", "a\0b", "a\u{7f}b"] {
            assert!(
                matches!(with_name(bad), Err(NameError::ForbiddenCharacter(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn a_leading_dot_is_rejected() {
        assert_eq!(with_name(".hidden"), Err(NameError::LeadingDot));
    }

    #[test]
    fn a_name_of_nothing_is_rejected() {
        let result = render_name(&NameParts {
            template: "",
            ..parts(ExportKind::Cycle)
        });

        assert_eq!(result, Err(NameError::Empty));
    }

    #[test]
    fn a_bad_ending_is_rejected() {
        let result = render_name(&NameParts {
            template: "{name}.",
            ..parts(ExportKind::Cycle)
        });

        assert_eq!(result, Err(NameError::BadEnding));
    }

    #[test]
    fn length_is_counted_after_normalising() {
        // Each decomposed character is 3 bytes, and 2 once composed.
        let fits = "e\u{301}".repeat(80);

        assert!(with_name(&fits).is_ok());
        assert_eq!(with_name(&"e".repeat(200)), Err(NameError::TooLong(221)),);
    }

    #[test]
    fn only_a_trailing_year_leaves_the_title() {
        let name = render_name(&NameParts {
            template: "{name} ({year}) {datetime}",
            name: "Example (2026)",
            ..parts(ExportKind::Cycle)
        })
        .unwrap();

        assert_eq!(name.title, name.name);
    }

    #[test]
    fn plain_folders_are_accepted() {
        assert_eq!(check_output_dir("Example TV"), Ok(()));
        assert_eq!(check_output_dir("Music/Example TV"), Ok(()));
    }

    #[test]
    fn folders_that_leave_the_library_are_rejected() {
        for bad in [
            "/srv/other",
            "../x",
            "a/../../x",
            "",
            ".",
            "a/./b",
            ".hidden",
            "a\\b",
        ] {
            assert!(check_output_dir(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn names_are_normalised() {
        let name = render_name(&NameParts {
            template: "{name}",
            name: "e\u{301}",
            ..parts(ExportKind::Cycle)
        })
        .unwrap();

        assert_eq!(name.name, "\u{e9}");
    }
}

//! Command-line entry point.

use anyhow::{anyhow, bail};
use clap::{Parser, Subcommand};
use dalsegno_app::acquisition::{Acquisition, AcquisitionOptions, FetchRequest, Library};
use dalsegno_app::analysis::PeriodConfig;
use dalsegno_app::backfill::DEFAULT_HOURS;
use dalsegno_app::config::Config;
use dalsegno_app::export::{
    AirTime, CutRequest, DEFAULT_SUFFIX, DEFAULT_TEMPLATE, ExportKind, NewProgram, Ownership,
    PlexTarget, PublishRequest, Redacted, TagEdits, find_tools,
};
use dalsegno_app::offline::{self, DiffRequest};
use serde::Serialize;
use std::path::PathBuf;
use text::{
    add_text, backfill_text, candidates_text, cut_text, diff_text, doctor_text, features_text,
    fetch_text, period_text, probe_text, publish_text, refine_text, scan_text, sweep_text,
};
use tracing_subscriber::filter::LevelFilter;

mod text;

/// Command-line interface for dalsegno.
#[derive(Debug, Parser)]
#[command(name = "dalsegno", version, about)]
struct Cli {
    /// Print machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,
    /// IP family policy: v6-first, v4-first, v6-only or v4-only.
    #[arg(
        long,
        global = true,
        env = "DALSEGNO_IP_POLICY",
        default_value = "v6-first"
    )]
    ip_policy: dalsegno_app::acquisition::IpPolicy,
    /// Cookie file for yt-dlp.
    #[arg(long, global = true, env = "DALSEGNO_COOKIES")]
    cookies: Option<PathBuf>,
    /// Directory that holds the database and fragments.
    #[arg(long, global = true, env = "DALSEGNO_DATA")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the service until it is stopped.
    Serve,
    /// Report tools, IPv6 route and IP family, and test URL resolution for a stream.
    Doctor {
        /// Video ID or URL to resolve over each family.
        #[arg(long)]
        stream: Option<String>,
    },
    /// Read a stream's metadata and choose its formats by checking real fragments.
    Probe {
        /// Video ID or URL.
        stream: String,
    },
    /// Probe a stream and register it.
    Add {
        /// Video ID or URL.
        stream: String,
    },
    /// Fetch a range of fragments. Stored fragments are skipped, so a rerun resumes.
    Fetch {
        /// Video ID or URL.
        stream: String,
        /// Format to fetch.
        #[arg(long)]
        itag: u32,
        /// First sq.
        #[arg(long)]
        first: u64,
        /// Last sq, inclusive.
        #[arg(long)]
        last: u64,
        /// Join the range into this file once it is complete.
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Fill the recent past of a stream's audio from the DVR, oldest first.
    Backfill {
        /// Video ID or URL.
        stream: String,
        /// How far back to reach.
        #[arg(long, default_value_t = DEFAULT_HOURS)]
        hours: f64,
    },
    /// Bring a stream's envelope up to date with its stored audio.
    Features {
        /// Video ID or URL.
        stream: String,
    },
    /// Remove fragments past retention and repair differences between files and rows.
    Sweep,
    /// Build an envelope file from numbered fragment files.
    Envelope {
        /// Directory of fragments named by sq.
        #[arg(long)]
        fragments: PathBuf,
        /// First sq.
        #[arg(long)]
        first: u64,
        /// Last sq, inclusive.
        #[arg(long)]
        last: u64,
        /// The `.npy` file to write.
        #[arg(long)]
        output: PathBuf,
    },
    /// Estimate the loop period of an envelope file.
    Period {
        /// Envelope file.
        #[arg(long)]
        env: PathBuf,
        /// Smallest period to consider, in seconds.
        #[arg(long)]
        min: Option<f64>,
        /// Largest period to consider, in seconds.
        #[arg(long)]
        max: Option<f64>,
    },
    /// Give a stream its program, which holds naming and the output folder.
    Program {
        /// Video ID or URL.
        stream: String,
        /// Short name, which starts each output name.
        #[arg(long)]
        short_name: String,
        /// Folder inside the library that receives output.
        #[arg(long)]
        output_dir: String,
        /// Template for output names.
        #[arg(long, default_value = DEFAULT_TEMPLATE)]
        template: String,
        /// Suffix of addition names.
        #[arg(long, default_value = DEFAULT_SUFFIX)]
        suffix: String,
        /// The `artist` tag.
        #[arg(long)]
        artist: Option<String>,
        /// The `date` tag, shared by everything from this stream.
        #[arg(long)]
        date: Option<String>,
        /// The `description` tag of a cycle.
        #[arg(long)]
        description: Option<String>,
    },
    /// List where a cut near a stream time could start and end. A person chooses.
    Candidates {
        /// Video ID or URL.
        stream: String,
        /// Stream time of the boundary, in seconds.
        #[arg(long)]
        time: f64,
    },
    /// Measure the period from black at a boundary and one cycle later.
    Refine {
        /// Video ID or URL.
        stream: String,
        /// Stream time of the boundary, in seconds.
        #[arg(long)]
        boundary: f64,
        /// The period from audio, in seconds.
        #[arg(long)]
        period: f64,
    },
    /// Write the finished folder of a cut in the work area.
    Cut {
        /// Video ID or URL.
        stream: String,
        /// What it holds.
        #[arg(long, value_enum)]
        kind: CutKind,
        /// The fragment whose start is the chosen keyframe.
        #[arg(long)]
        start_sq: u64,
        /// Length, in seconds. The end can be any frame.
        #[arg(long)]
        duration: f64,
        /// The moment the name refers to, in Unix seconds.
        #[arg(long)]
        named_at: i64,
        /// Stream time the version went on air, which a cycle must lie after.
        #[arg(long)]
        air_from: Option<f64>,
        /// Stream time the version was superseded.
        #[arg(long)]
        air_until: Option<f64>,
        /// Cut a cycle outside its air time, or with no air time given.
        #[arg(long)]
        override_air_time: bool,
        /// Replace the proposed `artist` tag.
        #[arg(long)]
        artist: Option<String>,
        /// Replace the proposed `date` tag.
        #[arg(long)]
        date: Option<String>,
        /// Replace the proposed `description` tag.
        #[arg(long)]
        description: Option<String>,
        /// Where the poster frame of an addition sits, in seconds from its start.
        #[arg(long)]
        poster_seconds: Option<f64>,
    },
    /// Move a finished folder into the library, and ask Plex for it.
    Publish {
        /// Clip row ID, which `cut` reports.
        clip: i64,
        /// Library root.
        #[arg(long, env = "DALSEGNO_LIBRARY")]
        library: PathBuf,
        /// Octal mode of the files.
        #[arg(long, default_value = "640", value_parser = parse_mode)]
        mode: u32,
        /// Owner of the files, as uid:gid.
        #[arg(long, value_parser = parse_owner)]
        owner: Option<(u32, u32)>,
        /// Plex base URL.
        #[arg(long, env = "DALSEGNO_PLEX_URL")]
        plex_url: Option<String>,
        /// Plex token.
        #[arg(long, env = "DALSEGNO_PLEX_TOKEN", hide_env_values = true)]
        plex_token: Option<String>,
        /// Plex library section.
        #[arg(long)]
        plex_section: Option<u32>,
    },
    /// Track the lag through an envelope file and report changes.
    Scan {
        /// Envelope file.
        #[arg(long)]
        env: PathBuf,
        /// The known period, in seconds.
        #[arg(long)]
        period: f64,
        /// Where to start, in seconds from the start of the file.
        #[arg(long)]
        from: Option<f64>,
    },
    /// Compare the last cycle of one envelope file with the last cycle of another.
    Diff {
        /// Envelope of the current version.
        #[arg(long)]
        env: PathBuf,
        /// Period of the current version, in seconds.
        #[arg(long)]
        period: f64,
        /// Envelope of the previous version.
        #[arg(long)]
        against: PathBuf,
        /// Period of the previous version, in seconds.
        #[arg(long)]
        against_period: f64,
        /// Envelope of a recurring clip.
        #[arg(long)]
        known: Option<PathBuf>,
    },
}

/// What a cut holds.
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum CutKind {
    /// The first cycle, named by the date it was acquired.
    FirstCycle,
    /// A later cycle, named by the moment the playlist switched.
    Cycle,
    /// Only what a version added.
    Addition,
}

impl From<CutKind> for ExportKind {
    fn from(kind: CutKind) -> Self {
        match kind {
            CutKind::FirstCycle => Self::FirstCycle,
            CutKind::Cycle => Self::Cycle,
            CutKind::Addition => Self::Addition,
        }
    }
}

fn parse_mode(text: &str) -> Result<u32, String> {
    u32::from_str_radix(text, 8)
        .ok()
        .filter(|mode| *mode <= 0o777)
        .ok_or_else(|| "expected an octal mode such as 640".to_string())
}

fn parse_owner(text: &str) -> Result<(u32, u32), String> {
    let parsed = text
        .split_once(':')
        .and_then(|(user, group)| Some((user.parse().ok()?, group.parse().ok()?)));
    parsed.ok_or_else(|| "expected uid:gid".to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_max_level(LevelFilter::WARN)
        .with_writer(std::io::stderr)
        .init();
    let Some(command) = cli.command else {
        return Ok(());
    };
    if matches!(command, Command::Serve) {
        return dalsegno_server::serve(&Config::load()?).await;
    }
    let json = cli.json;
    let command = match run_offline(command, json).await? {
        Some(command) => command,
        None => return Ok(()),
    };
    let acquisition = Acquisition::new(AcquisitionOptions {
        policy: cli.ip_policy,
        cookies: cli.cookies,
    })?;
    let data_dir = cli.data_dir;
    let library = || match &data_dir {
        Some(path) => Ok(Library::open(path)?),
        None => anyhow::Result::Err(anyhow!("this command needs --data-dir or DALSEGNO_DATA")),
    };
    match command {
        Command::Doctor { stream } => {
            let report = acquisition.doctor(stream.as_deref()).await?;
            print_report(cli.json, &report, doctor_text)
        }
        Command::Probe { stream } => {
            let report = acquisition.probe(&stream).await?;
            print_report(cli.json, &report, probe_text)
        }
        Command::Add { stream } => {
            let report = acquisition.add(&library()?, &stream).await?;
            print_report(cli.json, &report, add_text)
        }
        Command::Backfill { stream, hours } => {
            let report = acquisition.backfill(&library()?, &stream, hours).await?;
            print_report(cli.json, &report, backfill_text)
        }
        Command::Features { stream } => {
            let report = library()?.features(&stream).await?;
            print_report(cli.json, &report, features_text)
        }
        Command::Sweep => {
            let report = library()?.sweep().await?;
            print_report(cli.json, &report, sweep_text)
        }
        Command::Program {
            stream,
            short_name,
            output_dir,
            template,
            suffix,
            artist,
            date,
            description,
        } => {
            let id = library()?.set_program(
                &stream,
                &NewProgram {
                    short_name,
                    channel_id: None,
                    name_template: template,
                    addition_suffix: suffix,
                    artist,
                    content_date: date,
                    description,
                    output_dir,
                },
            )?;
            print_report(cli.json, &id, |id| format!("program: {id}\n"))
        }
        Command::Candidates { stream, time } => {
            let tools = find_tools()?;
            let report = acquisition
                .candidates(&library()?, &tools, &stream, time)
                .await?;
            print_report(cli.json, &report, candidates_text)
        }
        Command::Refine {
            stream,
            boundary,
            period,
        } => {
            let tools = find_tools()?;
            let report = acquisition
                .refine(&library()?, &tools, &stream, boundary, period)
                .await?;
            print_report(cli.json, &report, refine_text)
        }
        Command::Cut {
            stream,
            kind,
            start_sq,
            duration,
            named_at,
            air_from,
            air_until,
            override_air_time,
            artist,
            date,
            description,
            poster_seconds,
        } => {
            let tools = find_tools()?;
            let request = CutRequest {
                stream,
                kind: kind.into(),
                start_sq,
                duration,
                named_at,
                air_time: air_from.map(|from| AirTime {
                    from,
                    until: air_until,
                }),
                override_air_time,
                tags: TagEdits {
                    artist,
                    date,
                    description,
                },
                poster_seconds,
            };
            let report = acquisition.cut(&library()?, &tools, &request).await?;
            print_report(cli.json, &report, cut_text)
        }
        Command::Publish {
            clip,
            library: library_root,
            mode,
            owner,
            plex_url,
            plex_token,
            plex_section,
        } => {
            let plex = match (plex_url, plex_token, plex_section) {
                (Some(url), Some(token), Some(section)) => Some(PlexTarget {
                    url,
                    token: Redacted::new(token),
                    section,
                }),
                (None, None, None) => None,
                _ => bail!("Plex needs its URL, token and section together"),
            };
            let request = PublishRequest {
                clip,
                library_root,
                ownership: Ownership { mode, owner },
                plex,
            };
            let report = library()?.publish(&request).await?;
            print_report(cli.json, &report, publish_text)
        }
        // `run_offline` has already handled these.
        Command::Envelope { .. }
        | Command::Period { .. }
        | Command::Scan { .. }
        | Command::Diff { .. }
        | Command::Serve => Ok(()),
        Command::Fetch {
            stream,
            itag,
            first,
            last,
            output,
        } => {
            let request = FetchRequest {
                stream,
                itag,
                first,
                last,
                output,
            };
            let report = acquisition.fetch(&library()?, request).await?;
            print_report(cli.json, &report, fetch_text)?;
            if !report.is_complete() {
                bail!(
                    "range incomplete: {} lost, {} failed",
                    report.lost.len(),
                    report.failed.len()
                );
            }
            Ok(())
        }
    }
}

/// Runs a command that needs neither the network nor a library, and hands back any other.
async fn run_offline(command: Command, json: bool) -> anyhow::Result<Option<Command>> {
    let blocking = move || -> anyhow::Result<Option<Command>> {
        match command {
            Command::Envelope {
                fragments,
                first,
                last,
                output,
            } => {
                let report = offline::build_envelope(&fragments, first, last, &output)?;
                print_report(json, &report, |report| {
                    format!("fragments: {}\nbins: {}\n", report.fragments, report.bins)
                })?;
            }
            Command::Period { env, min, max } => {
                let defaults = PeriodConfig::default();
                let config = PeriodConfig {
                    min_period: min.unwrap_or(defaults.min_period),
                    max_period: max.unwrap_or(defaults.max_period),
                    ..defaults
                };
                print_report(json, &offline::period(&env, &config)?, period_text)?;
            }
            Command::Scan { env, period, from } => {
                print_report(json, &offline::scan(&env, period, from)?, scan_text)?;
            }
            Command::Diff {
                env,
                period,
                against,
                against_period,
                known,
            } => {
                let request = DiffRequest {
                    current: env,
                    period,
                    against,
                    against_period,
                    older: Vec::new(),
                    known,
                };
                print_report(json, &offline::diff(&request)?, diff_text)?;
            }
            other => return Ok(Some(other)),
        }
        Ok(None)
    };
    tokio::task::spawn_blocking(blocking).await?
}

fn print_report<T: Serialize>(
    json: bool,
    report: &T,
    text: fn(&T) -> String,
) -> anyhow::Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        print!("{}", text(report));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use clap::error::ErrorKind;
    use dalsegno_app::fetch::FetchReport;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn version_flag_is_reported() {
        let err = Cli::try_parse_from(["dalsegno", "--version"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
    }

    #[test]
    fn fetch_takes_a_range_and_an_output() {
        let cli = Cli::try_parse_from([
            "dalsegno",
            "fetch",
            "abcDEF123_-",
            "--itag",
            "140",
            "--first",
            "1",
            "--last",
            "2",
            "--data-dir",
            "/data",
            "--output",
            "/tmp/out",
            "--ip-policy",
            "v4-only",
        ])
        .unwrap();

        let Some(Command::Fetch {
            itag, last, output, ..
        }) = cli.command
        else {
            panic!("expected fetch");
        };
        assert_eq!((itag, last), (140, 2));
        assert_eq!(output, Some(PathBuf::from("/tmp/out")));
    }

    #[test]
    fn unknown_ip_policy_is_rejected() {
        let error = Cli::try_parse_from(["dalsegno", "--ip-policy", "ipv4", "doctor"]).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::ValueValidation);
    }

    #[test]
    fn fetch_text_counts_problems() {
        let report = FetchReport {
            fetched: 3,
            already_stored: 1,
            lost: vec![7],
            failed: vec![8, 9],
        };

        assert_eq!(
            fetch_text(&report),
            "fetched: 3\nalready stored: 1\nlost: 1\nfailed: 2\n"
        );
    }
}

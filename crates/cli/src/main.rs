//! Command-line entry point.

use anyhow::{anyhow, bail};
use clap::{Parser, Subcommand};
use dalsegno_app::acquisition::{Acquisition, AcquisitionOptions, Library};
use dalsegno_app::analysis::PeriodConfig;
use dalsegno_app::config::Config;
use dalsegno_app::export::{
    AirTime, CutRequest, ExportKind, Ownership, PlexTarget, PublishRequest, Redacted, TagEdits,
    find_tools,
};
use dalsegno_app::fetch::FetchLimits;
use dalsegno_app::offline::{self, DiffRequest};
use dalsegno_app::remote::ApiClient;
use remote::JobRequest;
use serde::Serialize;
use std::io::IsTerminal;
use std::path::PathBuf;
use text::{
    candidates_text, cut_text, diff_text, period_text, publish_text, refine_text, scan_text,
};
use tracing_subscriber::filter::LevelFilter;

mod catalog;
mod changes;
mod remote;
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
    /// Fragment requests started per second, shared by every stream and job.
    #[arg(
        long,
        global = true,
        env = "DALSEGNO_REQUESTS_PER_SECOND",
        default_value_t = FetchLimits::default().requests_per_second
    )]
    requests_per_second: f64,
    /// Fragment requests in flight at once, shared by every stream and job.
    #[arg(
        long,
        global = true,
        env = "DALSEGNO_CONCURRENCY",
        default_value_t = FetchLimits::default().concurrency
    )]
    concurrency: usize,
    /// Cookie file for yt-dlp.
    #[arg(long, global = true, env = "DALSEGNO_COOKIES")]
    cookies: Option<PathBuf>,
    /// Directory that holds the database and fragments.
    #[arg(long, global = true, env = "DALSEGNO_DATA")]
    data_dir: Option<PathBuf>,
    /// Address of the running service.
    #[arg(
        long,
        global = true,
        env = "DALSEGNO_URL",
        default_value = "http://127.0.0.1:8737"
    )]
    url: String,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the service until it is stopped.
    Serve,
    /// Show the streams of the running service.
    Status,
    /// Show the newest jobs of the running service.
    Jobs,
    /// Show the newest changes of the running service.
    Changes,
    /// Start following a stream, and filling what it missed.
    Start {
        /// Video ID or URL.
        stream: String,
    },
    /// Stop following a stream.
    Stop {
        /// Video ID or URL.
        stream: String,
    },
    /// Report tools, IPv6 route and IP family from the running service, and with a stream
    /// resolve its URL over each family and probe its formats.
    Doctor {
        /// Video ID or URL to resolve over each family and probe.
        #[arg(long)]
        stream: Option<String>,
    },
    /// Show the shared settings, after changing them when given KEY=VALUE or --reset. With
    /// --stream or --program it changes that stream's title or that program's fields instead.
    Set {
        /// What to change, as KEY=VALUE, such as concurrency=2, daily_summary=09:00 or, with
        /// --stream, title=Example.
        pairs: Vec<String>,
        /// Setting whose override to drop.
        #[arg(long = "reset", value_name = "KEY")]
        resets: Vec<String>,
        /// Change the title of this stream, by video ID or URL.
        #[arg(long, conflicts_with_all = ["program", "resets"])]
        stream: Option<String>,
        /// Change fields of this program, by ID, such as short_name=Example or artist=Example.
        #[arg(long, conflicts_with_all = ["stream", "resets"])]
        program: Option<i64>,
    },
    /// Probe a stream and register it, which can take a few minutes.
    Add {
        /// Video ID or URL.
        stream: String,
    },
    /// Queue a fetch of a range of fragments. Stored fragments are skipped, so a rerun resumes.
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
    },
    /// Queue filling the recent past of a stream's audio from the DVR, oldest first.
    Backfill {
        /// Video ID or URL.
        stream: String,
        /// How far back to reach. The service default is 24 hours.
        #[arg(long)]
        hours: Option<f64>,
    },
    /// Queue bringing a stream's envelope up to date with its stored audio.
    Features {
        /// Video ID or URL.
        stream: String,
    },
    /// Queue removing fragments past retention and repairing differences between files and rows.
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
        #[arg(long)]
        template: Option<String>,
        /// Suffix of addition names.
        #[arg(long)]
        suffix: Option<String>,
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
    /// Show the programs, with the streams linked to each.
    Programs,
    /// Make a stream part of a program.
    Link {
        /// Video ID or URL.
        stream: String,
        /// Program ID.
        program: i64,
    },
    /// Show a program's versions, newest first.
    Versions {
        /// Program ID.
        program: i64,
    },
    /// List a program's known clips, or with --add or --remove change them.
    Known {
        /// Program ID.
        program: i64,
        /// Name of a clip to add, cut from the stored envelope of --stream.
        #[arg(long, requires_all = ["stream", "from", "to"], conflicts_with = "remove")]
        add: Option<String>,
        /// Video ID or URL of the stream to cut from.
        #[arg(long, requires = "add")]
        stream: Option<String>,
        /// Stream time the clip starts, in seconds.
        #[arg(long, requires = "add")]
        from: Option<f64>,
        /// Stream time the clip ends, in seconds.
        #[arg(long, requires = "add")]
        to: Option<f64>,
        /// ID of a clip to remove.
        #[arg(long)]
        remove: Option<i64>,
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
    /// Show both versions of a change from the service, or with --env compare the last cycle of
    /// one envelope file with the last cycle of another.
    Diff {
        /// Change ID, when asking the service.
        #[arg(required_unless_present = "env", conflicts_with = "env")]
        change: Option<i64>,
        /// Envelope of the current version.
        #[arg(long, requires_all = ["period", "against", "against_period"])]
        env: Option<PathBuf>,
        /// Period of the current version, in seconds.
        #[arg(long, requires = "env")]
        period: Option<f64>,
        /// Envelope of the previous version.
        #[arg(long, requires = "env")]
        against: Option<PathBuf>,
        /// Period of the previous version, in seconds.
        #[arg(long, requires = "env")]
        against_period: Option<f64>,
        /// Envelope of a recurring clip.
        #[arg(long, requires = "env")]
        known: Option<PathBuf>,
    },
    /// Close a change on your word. Its holds lapse after a week.
    Dismiss {
        /// Change ID.
        change: i64,
    },
    /// Start a draft clip from a change, or show a clip, or with --kind, --from or --to reshape
    /// a draft.
    Clip {
        /// Clip ID, to show or reshape.
        #[arg(required_unless_present = "change", conflicts_with = "change")]
        clip: Option<i64>,
        /// Change to start a clip from.
        #[arg(long, requires_all = ["kind", "from", "to"])]
        change: Option<i64>,
        /// What it holds.
        #[arg(long, value_enum)]
        kind: Option<changes::ClipKind>,
        /// Stream time it starts, in seconds.
        #[arg(long)]
        from: Option<f64>,
        /// Stream time it ends, in seconds.
        #[arg(long)]
        to: Option<f64>,
        /// Version whose cycle the clip is cut from, when it is a cycle.
        #[arg(long, requires = "change")]
        version: Option<i64>,
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

/// Runs what goes through the service, and hands back what does not.
async fn run_remote(command: Command, url: &str, json: bool) -> anyhow::Result<Option<Command>> {
    let client = || ApiClient::new(url);
    match command {
        Command::Status => remote::status(&client()?, json).await?,
        Command::Jobs => remote::jobs(&client()?, json).await?,
        Command::Changes => remote::changes(&client()?, json).await?,
        Command::Start { stream } => {
            remote::stream_action(&client()?, &stream, "start", json).await?;
        }
        Command::Stop { stream } => {
            remote::stream_action(&client()?, &stream, "stop", json).await?;
        }
        Command::Doctor { stream } => {
            remote::doctor(&client()?, stream.as_deref(), json).await?;
        }
        Command::Set {
            pairs,
            resets,
            stream,
            program,
        } => match (stream, program) {
            (Some(stream), _) => catalog::set_stream(&client()?, &stream, &pairs, json).await?,
            (None, Some(program)) => {
                catalog::set_program(&client()?, program, &pairs, json).await?;
            }
            (None, None) => remote::set(&client()?, &pairs, &resets, json).await?,
        },
        Command::Add { stream } => catalog::add(&client()?, &stream, json).await?,
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
            let args = catalog::ProgramArgs {
                short_name,
                output_dir,
                template,
                suffix,
                artist,
                date,
                description,
            };
            catalog::program(&client()?, &stream, &args, json).await?;
        }
        Command::Programs => catalog::programs(&client()?, json).await?,
        Command::Link { stream, program } => {
            catalog::link(&client()?, &stream, program, json).await?;
        }
        Command::Versions { program } => catalog::versions(&client()?, program, json).await?,
        Command::Known {
            program,
            add,
            stream,
            from,
            to,
            remove,
        } => {
            let action = match (add, stream, from, to, remove) {
                (Some(name), Some(stream), Some(from), Some(to), None) => {
                    catalog::KnownAction::Add {
                        name,
                        stream,
                        from,
                        to,
                    }
                }
                (None, _, _, _, Some(clip)) => catalog::KnownAction::Remove(clip),
                _ => catalog::KnownAction::List,
            };
            catalog::known(&client()?, program, &action, json).await?;
        }
        Command::Backfill { stream, hours } => {
            let request = JobRequest::Backfill {
                stream: &stream,
                hours,
            };
            remote::queue(&client()?, &request, json).await?;
        }
        Command::Features { stream } => {
            let request = JobRequest::Features { stream: &stream };
            remote::queue(&client()?, &request, json).await?;
        }
        Command::Sweep => remote::queue(&client()?, &JobRequest::Sweep, json).await?,
        Command::Diff {
            change: Some(change),
            ..
        } => changes::diff(&client()?, change, json).await?,
        Command::Dismiss { change } => changes::dismiss(&client()?, change, json).await?,
        Command::Clip {
            clip,
            change,
            kind,
            from,
            to,
            version,
        } => {
            let action = match (clip, change, kind, from, to) {
                (None, Some(change), Some(kind), Some(from), Some(to)) => {
                    changes::ClipAction::Create {
                        change,
                        kind,
                        from,
                        to,
                        version,
                    }
                }
                (Some(clip), _, None, None, None) => changes::ClipAction::Show(clip),
                (Some(clip), _, kind, from, to) => changes::ClipAction::Edit {
                    clip,
                    kind,
                    from,
                    to,
                },
                _ => bail!("give a clip, or --change with --kind, --from and --to"),
            };
            changes::clip(&client()?, &action, json).await?;
        }
        Command::Fetch {
            stream,
            itag,
            first,
            last,
        } => {
            let request = JobRequest::Fetch {
                stream: &stream,
                itag,
                first,
                last,
            };
            remote::queue(&client()?, &request, json).await?;
        }
        other => return Ok(Some(other)),
    }
    Ok(None)
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

/// The service says what it does, so a stalled one can be told from a healthy one. Other
/// commands print their own report and log only problems.
fn log_level(command: Option<&Command>) -> LevelFilter {
    match command {
        Some(Command::Serve) => LevelFilter::INFO,
        _ => LevelFilter::WARN,
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_max_level(log_level(cli.command.as_ref()))
        .with_ansi(std::io::stderr().is_terminal())
        .with_writer(std::io::stderr)
        .init();
    let Some(command) = cli.command else {
        return Ok(());
    };
    let limits = FetchLimits {
        concurrency: cli.concurrency,
        requests_per_second: cli.requests_per_second,
    };
    if matches!(command, Command::Serve) {
        return dalsegno_server::serve(&Config::load()?, cli.ip_policy, limits).await;
    }
    let json = cli.json;
    let command = match run_offline(command, json).await? {
        Some(command) => command,
        None => return Ok(()),
    };
    let command = match run_remote(command, &cli.url, json).await? {
        Some(command) => command,
        None => return Ok(()),
    };
    let acquisition = Acquisition::new(AcquisitionOptions {
        policy: cli.ip_policy,
        cookies: cli.cookies,
        limits,
    })?;
    let data_dir = cli.data_dir;
    let library = || match &data_dir {
        Some(path) => Ok(Library::open(path)?),
        None => anyhow::Result::Err(anyhow!("this command needs --data-dir or DALSEGNO_DATA")),
    };
    match command {
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
        | Command::Serve => Ok(()),
        // `run_remote` has already handled these.
        Command::Status
        | Command::Jobs
        | Command::Changes
        | Command::Start { .. }
        | Command::Stop { .. }
        | Command::Doctor { .. }
        | Command::Set { .. }
        | Command::Add { .. }
        | Command::Program { .. }
        | Command::Programs
        | Command::Link { .. }
        | Command::Versions { .. }
        | Command::Known { .. }
        | Command::Backfill { .. }
        | Command::Features { .. }
        | Command::Sweep
        | Command::Diff { .. }
        | Command::Dismiss { .. }
        | Command::Clip { .. }
        | Command::Fetch { .. } => Ok(()),
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
                env: Some(env),
                period: Some(period),
                against: Some(against),
                against_period: Some(against_period),
                known,
                ..
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
mod tests;

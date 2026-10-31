//! Command-line entry point.

use anyhow::{anyhow, bail};
use clap::{Parser, Subcommand};
use dalsegno_app::acquisition::{
    Acquisition, AcquisitionOptions, AddReport, FetchRequest, Library,
};
use dalsegno_app::analysis::{Diff, PeriodConfig};
use dalsegno_app::backfill::{BackfillReport, DEFAULT_HOURS};
use dalsegno_app::doctor::DoctorReport;
use dalsegno_app::features::FeatureReport;
use dalsegno_app::fetch::FetchReport;
use dalsegno_app::offline::{self, DiffRequest, PeriodReport, ScanReport};
use dalsegno_app::probe::ProbeReport;
use dalsegno_app::retention::SweepReport;
use serde::Serialize;
use std::path::PathBuf;
use tracing_subscriber::filter::LevelFilter;

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
        // `run_offline` has already handled these.
        Command::Envelope { .. }
        | Command::Period { .. }
        | Command::Scan { .. }
        | Command::Diff { .. } => Ok(()),
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

fn period_text(report: &PeriodReport) -> String {
    match (report.period, report.testable_up_to) {
        (Some(period), _) => format!(
            "period: {period:.1} s\nagreeing references: {}\nmatch at double: {}\n",
            report.agreeing, report.confirmed_by_double
        ),
        (None, Some(limit)) => format!("no period found, testable up to {limit:.0} s\n"),
        (None, None) => "no period found\n".to_string(),
    }
}

fn scan_text(report: &ScanReport) -> String {
    let runs = report.runs.iter().map(|run| {
        let lag = run
            .lag
            .map_or(String::new(), |lag| format!(" lag {lag:.1} s"));
        format!(
            "{:.0}..{:.0} {:?}{lag} ({} points)\n",
            run.start, run.end, run.kind, run.points
        )
    });
    let signals = report
        .signals
        .iter()
        .map(|signal| format!("signal: {signal:?}\n"));
    runs.chain(signals).collect()
}

fn diff_text(report: &Diff) -> String {
    let regions = report.regions.iter().map(|region| {
        let moved = if region.moved { " (moved)" } else { "" };
        format!(
            "{:?}{moved} {:.0}..{:.0} ({:.0} s)\n",
            region.class,
            region.start,
            region.end,
            region.end - region.start
        )
    });
    let checks = report.checks.iter().map(|check| {
        let result = if check.passed { "ok" } else { "REVIEW" };
        format!(
            "{result}: {}: expected {:.1}, measured {:.1}\n",
            check.name, check.expected, check.actual
        )
    });
    regions.chain(checks).collect()
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

fn or_none(value: Option<&str>) -> &str {
    value.unwrap_or("none")
}

fn doctor_text(report: &DoctorReport) -> String {
    let tools = report.tools.iter().map(|tool| {
        format!(
            "{}: {} ({})\n",
            tool.name,
            or_none(tool.path.as_deref()),
            or_none(tool.version.as_deref())
        )
    });
    let switch = report.last_switch.map_or("none".to_string(), |switch| {
        format!(
            "{} to {} at {}, {:?}",
            switch.from, switch.to, switch.at, switch.reason
        )
    });
    let resolution = report.resolution.iter().map(|check| {
        format!(
            "resolve over {}: {}\n",
            check.family,
            check.error.as_deref().unwrap_or("ok")
        )
    });
    let family = format!(
        "ipv6 route: {}\nip policy: {:?}\nactive family: {}\nlast switch: {switch}\n",
        report.has_ipv6_route, report.policy, report.active_family
    );
    tools.chain([family]).chain(resolution).collect()
}

fn probe_text(report: &ProbeReport) -> String {
    let format_line = |format: &Option<dalsegno_app::probe::FormatInfo>| {
        format.as_ref().map_or("none".to_string(), |format| {
            let codec = format.vcodec.as_ref().or(format.acodec.as_ref());
            format!(
                "itag {} ({}, {}p)",
                format.itag,
                or_none(codec.map(String::as_str)),
                format.height.unwrap_or(0)
            )
        })
    };
    let summary = format!(
        "title: {}\nshort name: {}\nchannel: {}\nfamily: {}\nhead sq: {}\nfragment length: {}\n\
         audio: {}\nvideo: {}\n",
        report.title,
        report.short_name,
        or_none(report.channel.as_deref()),
        report.family,
        report.head_sq,
        report
            .fragment_seconds
            .map_or("unknown".to_string(), |seconds| format!("{seconds:.3} s")),
        format_line(&report.audio),
        format_line(&report.video),
    );
    let rejected = report
        .rejected
        .iter()
        .map(|rejection| format!("rejected itag {}: {}\n", rejection.itag, rejection.reason));
    std::iter::once(summary).chain(rejected).collect()
}

fn add_text(report: &AddReport) -> String {
    format!(
        "stream id: {}\n{}",
        report.stream_id,
        probe_text(&report.probe)
    )
}

fn backfill_text(report: &BackfillReport) -> String {
    format!(
        "head sq: {}\nfetched: {}\nlost: {}\nstill open: {}\n",
        report.head_sq, report.fetched, report.lost, report.failed
    )
}

fn features_text(report: &FeatureReport) -> String {
    format!(
        "fragments: {}\nbins: {}\nruns: {}\nunreadable: {}\n",
        report.fragments,
        report.bins,
        report.runs,
        report.unreadable.len()
    )
}

fn sweep_text(report: &SweepReport) -> String {
    format!(
        "expired: {}\norphans: {}\ntemporary: {}\nrepaired rows: {}\n",
        report.expired, report.orphans, report.temporary, report.repaired
    )
}

fn fetch_text(report: &FetchReport) -> String {
    format!(
        "fetched: {}\nalready stored: {}\nlost: {}\nfailed: {}\n",
        report.fetched,
        report.already_stored,
        report.lost.len(),
        report.failed.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use clap::error::ErrorKind;

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

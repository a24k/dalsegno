//! Command-line entry point.

use anyhow::bail;
use clap::{Parser, Subcommand};
use dalsegno_app::acquisition::{Acquisition, AcquisitionOptions, FetchRequest};
use dalsegno_app::doctor::DoctorReport;
use dalsegno_app::fetch::FetchReport;
use dalsegno_app::probe::ProbeReport;
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
        /// Directory that holds fragments.
        #[arg(long, env = "DALSEGNO_DATA")]
        data_dir: PathBuf,
        /// Join the range into this file once it is complete.
        #[arg(long)]
        output: Option<PathBuf>,
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
    let acquisition = Acquisition::new(AcquisitionOptions {
        policy: cli.ip_policy,
        cookies: cli.cookies,
    })?;
    match command {
        Command::Doctor { stream } => {
            let report = acquisition.doctor(stream.as_deref()).await?;
            print_report(cli.json, &report, doctor_text)
        }
        Command::Probe { stream } => {
            let report = acquisition.probe(&stream).await?;
            print_report(cli.json, &report, probe_text)
        }
        Command::Fetch {
            stream,
            itag,
            first,
            last,
            data_dir,
            output,
        } => {
            let request = FetchRequest {
                stream,
                itag,
                first,
                last,
                data_dir,
                output,
            };
            let report = acquisition.fetch(request).await?;
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

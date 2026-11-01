//! Plain-text reports.

use dalsegno_app::acquisition::AddReport;
use dalsegno_app::analysis::Diff;
use dalsegno_app::backfill::BackfillReport;
use dalsegno_app::doctor::DoctorReport;
use dalsegno_app::export::{CandidateReport, CutReport, PublishReport, RefineReport};
use dalsegno_app::features::FeatureReport;
use dalsegno_app::fetch::FetchReport;
use dalsegno_app::offline::{PeriodReport, ScanReport};
use dalsegno_app::probe::ProbeReport;
use dalsegno_app::retention::SweepReport;

pub(crate) fn candidates_text(report: &CandidateReport) -> String {
    let starts: String = report
        .starts
        .iter()
        .map(|start| {
            format!(
                "start sq {} at {:.3} ({:?})\n",
                start.keyframe.sq, start.keyframe.time, start.evidence
            )
        })
        .collect();
    let ends: String = report
        .ends
        .iter()
        .map(|end| {
            format!(
                "end at {:.3} ({:?}, {:?})\n",
                end.time, end.evidence, end.edge
            )
        })
        .collect();
    format!(
        "{starts}{ends}contact sheet: {}\n",
        report.contact_sheet.display()
    )
}

pub(crate) fn refine_text(report: &RefineReport) -> String {
    match report.video {
        Some(video) => format!(
            "audio: {:.3}\nvideo: {:.3} from {} edges\n",
            report.audio, video.period, video.agreeing
        ),
        None => format!("audio: {:.3}\nvideo: no black agrees\n", report.audio),
    }
}

pub(crate) fn cut_text(report: &CutReport) -> String {
    format!(
        "clip: {}\nname: {}\nfolder: {}\nfragments: {}..={}\nduration: {:.3}\n",
        report.clip_id,
        report.name,
        report.folder.display(),
        report.range.sq_first,
        report.range.sq_last,
        report.duration
    )
}

pub(crate) fn publish_text(report: &PublishReport) -> String {
    let plex = report.plex.as_deref().unwrap_or("not asked");
    format!(
        "target: {}\nalready there: {}\nplex: {plex}\n",
        report.target.display(),
        report.already_there
    )
}

pub(crate) fn period_text(report: &PeriodReport) -> String {
    match (report.period, report.testable_up_to) {
        (Some(period), _) => format!(
            "period: {period:.1} s\nagreeing references: {}\nmatch at double: {}\n",
            report.agreeing, report.confirmed_by_double
        ),
        (None, Some(limit)) => format!("no period found, testable up to {limit:.0} s\n"),
        (None, None) => "no period found\n".to_string(),
    }
}

pub(crate) fn scan_text(report: &ScanReport) -> String {
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

pub(crate) fn diff_text(report: &Diff) -> String {
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

fn or_none(value: Option<&str>) -> &str {
    value.unwrap_or("none")
}

pub(crate) fn doctor_text(report: &DoctorReport) -> String {
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

pub(crate) fn probe_text(report: &ProbeReport) -> String {
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

pub(crate) fn add_text(report: &AddReport) -> String {
    format!(
        "stream id: {}\n{}",
        report.stream_id,
        probe_text(&report.probe)
    )
}

pub(crate) fn backfill_text(report: &BackfillReport) -> String {
    format!(
        "head sq: {}\nfetched: {}\nlost: {}\nstill open: {}\n",
        report.head_sq, report.fetched, report.lost, report.failed
    )
}

pub(crate) fn features_text(report: &FeatureReport) -> String {
    format!(
        "fragments: {}\nbins: {}\nruns: {}\nunreadable: {}\n",
        report.fragments,
        report.bins,
        report.runs,
        report.unreadable.len()
    )
}

pub(crate) fn sweep_text(report: &SweepReport) -> String {
    format!(
        "expired: {}\norphans: {}\ntemporary: {}\nrepaired rows: {}\n",
        report.expired, report.orphans, report.temporary, report.repaired
    )
}

pub(crate) fn fetch_text(report: &FetchReport) -> String {
    format!(
        "fetched: {}\nalready stored: {}\nlost: {}\nfailed: {}\n",
        report.fetched,
        report.already_stored,
        report.lost.len(),
        report.failed.len()
    )
}

//! HTTP API, scheduler, job runner and stream supervisors.

use api::{ApiState, Services, router};
use dalsegno_app::acquisition::{Acquisition, AcquisitionOptions, FetchRequest, IpPolicy, Library};
use dalsegno_app::backfill::DEFAULT_HOURS;
use dalsegno_app::config::Config;
use dalsegno_app::fetch::FetchLimits;
use dalsegno_app::lifecycle::{
    ANALYSIS, BACKFILL, FEATURES, FETCH, FetchTarget, PREFETCH, PrefetchTarget, StreamTarget,
};
use dalsegno_app::settings::{LiveSettings, Operational};
use dalsegno_io::webhook::Webhook;
use dalsegno_store::jobs::JobRow;
use dalsegno_store::streams::StreamRow;
use notifier::Notifier;
use runner::{JobFuture, JobHandler, JobRunner, record_progress};
use scheduler::{SWEEP, run_scheduler};
use std::sync::Arc;
use supervisor::{FollowFuture, Follower, Supervisor};
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::watch;

pub mod api;
mod catalog;
mod changes;
pub mod events;
mod known;
pub mod notifier;
mod operations;
pub mod runner;
pub mod scheduler;
pub mod supervisor;
mod ui;

/// The real adapters behind the jobs and the follow tasks.
#[derive(Debug, Clone)]
struct Service {
    acquisition: Acquisition,
    library: Library,
}

impl Service {
    async fn run_job(self, job: JobRow) -> Result<(), String> {
        let kind = job.job.kind.as_str();
        if kind == SWEEP {
            let report = self.library.sweep().await;
            return report.map(drop).map_err(|error| error.to_string());
        }
        let invalid = || format!("the target of job {} is not valid", job.id);
        if kind == FETCH {
            let target = FetchTarget::parse(&job.job.target).ok_or_else(invalid)?;
            return self.fetch(target).await;
        }
        if kind == PREFETCH {
            let target = PrefetchTarget::parse(&job.job.target).ok_or_else(invalid)?;
            let done = self.acquisition.prefetch(&self.library, &target).await;
            return done.map_err(|error| error.to_string());
        }
        let target = StreamTarget::parse(&job.job.target).ok_or_else(invalid)?;
        let stream = target.stream.as_str();
        let done = match kind {
            BACKFILL => {
                let database = self.library.database();
                let progress = |progress| record_progress(&database, job.id, progress);
                let hours = target.hours.unwrap_or(DEFAULT_HOURS);
                self.acquisition
                    .backfill(&self.library, stream, hours, &progress)
                    .await
                    .map(drop)
            }
            FEATURES => self.library.features(stream).await.map(drop),
            ANALYSIS => self.library.analyse(stream).await.map(drop),
            other => return Err(format!("unknown job kind {other}")),
        };
        done.map_err(|error| error.to_string())
    }
}

impl Service {
    /// Fetches a range into the library, as `dalsegno fetch` asked, and fails unless all of
    /// it arrived.
    async fn fetch(&self, target: FetchTarget) -> Result<(), String> {
        let request = FetchRequest {
            stream: target.stream,
            itag: target.itag,
            first: target.first,
            last: target.last,
            output: None,
        };
        let report = self
            .acquisition
            .fetch(&self.library, request)
            .await
            .map_err(|error| error.to_string())?;
        if report.is_complete() {
            return Ok(());
        }
        Err(format!(
            "range incomplete: {} lost, {} failed",
            report.lost.len(),
            report.failed.len()
        ))
    }
}

/// Hands each change of the request budget to the shared scheduler until the service stops.
async fn apply_limits(
    mut settings: watch::Receiver<Operational>,
    acquisition: Acquisition,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            changed = settings.changed() => {
                if changed.is_err() {
                    break;
                }
                let limits = settings.borrow_and_update().limits;
                if let Err(error) = acquisition.set_limits(limits) {
                    tracing::warn!(error = %error, "the new request budget was not applied");
                }
            }
            _ = shutdown.wait_for(|is_stopping| *is_stopping) => break,
        }
    }
}

impl JobHandler for Service {
    fn run(&self, job: JobRow) -> JobFuture {
        Box::pin(self.clone().run_job(job))
    }
}

impl Follower for Service {
    fn follow(&self, row: StreamRow, stop: watch::Receiver<bool>) -> FollowFuture {
        let service = self.clone();
        Box::pin(async move {
            service
                .acquisition
                .follow(&service.library, &row, stop)
                .await
                .map_err(|error| error.to_string())
        })
    }
}

async fn stopped(mut shutdown: watch::Receiver<bool>) {
    let _ = shutdown.wait_for(|is_stopping| *is_stopping).await;
}

/// Runs the service until it is told to stop. Every stream and job shares `limits`.
pub async fn serve(config: &Config, policy: IpPolicy, limits: FetchLimits) -> anyhow::Result<()> {
    let library = Library::open(&config.data)?;
    let database = library.database();
    let acquisition = Acquisition::new(AcquisitionOptions {
        policy,
        cookies: config.cookies.clone(),
        limits,
    })?;
    let base = Operational {
        limits,
        daily_summary: config.daily_summary,
    };
    let (settings, left_off) = LiveSettings::load(base, database.clone())?;
    acquisition.set_limits(settings.current().limits)?;
    let ignored: Vec<String> = config
        .ignored
        .iter()
        .cloned()
        .chain(
            left_off
                .iter()
                .map(|key| format!("the stored {key} setting")),
        )
        .collect();
    let listener = TcpListener::bind(config.bind).await?;
    // The image binds every address inside its container and leaves the limit to the published
    // port, so this is a reminder at start rather than a warning on every run.
    if !config.bind.ip().is_loopback() {
        tracing::info!(
            bind = %config.bind,
            "the API has no login of its own, so anything beyond loopback needs an \
             authenticating proxy in front (D-20)"
        );
    }
    let services = Services {
        acquisition: acquisition.clone(),
        settings: settings.clone(),
        bootstrap: config.bootstrap_view(),
    };
    let state = ApiState::new(library.clone(), config.public_url.as_deref(), services);
    let bus = state.events();
    let api = router(state);
    let service = Arc::new(Service {
        acquisition: acquisition.clone(),
        library,
    });
    let runner = Arc::new(JobRunner::new(database.clone(), service.clone()));
    let requeued = runner.recover().map_err(anyhow::Error::msg)?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        requeued,
        "service starting"
    );
    for problem in &ignored {
        tracing::warn!(problem, "setting ignored");
    }
    if let Err(error) = notifier::announce_start(&database, requeued, &ignored) {
        tracing::warn!(error, "the start was not announced");
    }
    let (stop, shutdown) = watch::channel(false);
    let jobs = tokio::spawn(runner.run(shutdown.clone()));
    let schedule = tokio::spawn(run_scheduler(
        database.clone(),
        settings.subscribe(),
        shutdown.clone(),
    ));
    let budget = tokio::spawn(apply_limits(
        settings.subscribe(),
        acquisition,
        shutdown.clone(),
    ));
    let notices = config
        .webhook
        .clone()
        .map(|url| Webhook::new(url, config.webhook_format))
        .transpose()?
        .map(|hook| {
            let notifier =
                Notifier::new(database.clone(), Arc::new(hook), config.public_url.clone());
            tokio::spawn(notifier.run(shutdown.clone()))
        });
    let updates = tokio::spawn(events::run_watcher(
        database.clone(),
        bus,
        events::POLL_INTERVAL,
        shutdown.clone(),
    ));
    let streams = tokio::spawn(Supervisor::new(database, service).run(shutdown.clone()));
    let http = tokio::spawn(
        axum::serve(listener, api)
            .with_graceful_shutdown(stopped(shutdown))
            .into_future(),
    );
    signal::ctrl_c().await?;
    tracing::info!("service stopping");
    stop.send(true)?;
    http.await??;
    updates.await?;
    streams.await?;
    jobs.await?;
    schedule.await?;
    budget.await?;
    if let Some(notices) = notices {
        notices.await?;
    }
    Ok(())
}

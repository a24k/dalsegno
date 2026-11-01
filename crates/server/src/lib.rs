//! HTTP API, scheduler, job runner and stream supervisors.

use api::{ApiState, router};
use dalsegno_app::acquisition::{Acquisition, AcquisitionOptions, IpPolicy, Library};
use dalsegno_app::backfill::DEFAULT_HOURS;
use dalsegno_app::config::Config;
use dalsegno_app::lifecycle::{
    ANALYSIS, BACKFILL, FEATURES, PREFETCH, PrefetchTarget, StreamTarget,
};
use dalsegno_io::webhook::Webhook;
use dalsegno_store::jobs::JobRow;
use dalsegno_store::streams::StreamRow;
use notifier::Notifier;
use runner::{JobFuture, JobHandler, JobRunner};
use scheduler::{SWEEP, run_scheduler};
use std::sync::Arc;
use supervisor::{FollowFuture, Follower, Supervisor};
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::watch;

pub mod api;
pub mod notifier;
pub mod runner;
pub mod scheduler;
pub mod supervisor;

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
        if kind == PREFETCH {
            let target = PrefetchTarget::parse(&job.job.target).ok_or_else(invalid)?;
            let done = self.acquisition.prefetch(&self.library, &target).await;
            return done.map_err(|error| error.to_string());
        }
        let target = StreamTarget::parse(&job.job.target).ok_or_else(invalid)?;
        let stream = target.stream.as_str();
        let done = match kind {
            BACKFILL => self
                .acquisition
                .backfill(&self.library, stream, DEFAULT_HOURS)
                .await
                .map(drop),
            FEATURES => self.library.features(stream).await.map(drop),
            ANALYSIS => self.library.analyse(stream).await.map(drop),
            other => return Err(format!("unknown job kind {other}")),
        };
        done.map_err(|error| error.to_string())
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

/// Runs the service until it is told to stop.
pub async fn serve(config: &Config, policy: IpPolicy) -> anyhow::Result<()> {
    let library = Library::open(&config.data)?;
    let database = library.database();
    let acquisition = Acquisition::new(AcquisitionOptions {
        policy,
        cookies: config.cookies.clone(),
    })?;
    let listener = TcpListener::bind(config.bind).await?;
    let api = router(ApiState::new(library.clone()));
    let service = Arc::new(Service {
        acquisition,
        library,
    });
    let runner = Arc::new(JobRunner::new(database.clone(), service.clone()));
    let recovered = runner.recover().map_err(anyhow::Error::msg)?;
    tracing::info!(recovered, "service starting");
    let (stop, shutdown) = watch::channel(false);
    let jobs = tokio::spawn(runner.run(shutdown.clone()));
    let schedule = tokio::spawn(run_scheduler(database.clone(), shutdown.clone()));
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
    streams.await?;
    jobs.await?;
    schedule.await?;
    if let Some(notices) = notices {
        notices.await?;
    }
    Ok(())
}

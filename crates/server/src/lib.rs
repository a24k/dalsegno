//! HTTP API, scheduler, job runner and stream supervisors.

use dalsegno_app::acquisition::{Acquisition, AcquisitionOptions, IpPolicy, Library};
use dalsegno_app::backfill::DEFAULT_HOURS;
use dalsegno_app::config::Config;
use dalsegno_app::lifecycle::{BACKFILL, FEATURES, StreamTarget};
use dalsegno_store::jobs::JobRow;
use dalsegno_store::streams::StreamRow;
use runner::{JobFuture, JobHandler, JobRunner};
use scheduler::{SWEEP, run_scheduler};
use std::sync::Arc;
use supervisor::{FollowFuture, Follower, Supervisor};
use tokio::signal;
use tokio::sync::watch;

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
        let target = StreamTarget::parse(&job.job.target)
            .ok_or_else(|| format!("the target of job {} is not valid", job.id))?;
        let stream = target.stream.as_str();
        let done = match kind {
            BACKFILL => self
                .acquisition
                .backfill(&self.library, stream, DEFAULT_HOURS)
                .await
                .map(drop),
            FEATURES => self.library.features(stream).await.map(drop),
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

/// Runs the service until it is told to stop.
pub async fn serve(config: &Config, policy: IpPolicy) -> anyhow::Result<()> {
    let library = Library::open(&config.data)?;
    let database = library.database();
    let acquisition = Acquisition::new(AcquisitionOptions {
        policy,
        cookies: config.cookies.clone(),
    })?;
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
    let streams = tokio::spawn(Supervisor::new(database, service).run(shutdown));
    signal::ctrl_c().await?;
    tracing::info!("service stopping");
    stop.send(true)?;
    streams.await?;
    jobs.await?;
    schedule.await?;
    Ok(())
}

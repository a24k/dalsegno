//! HTTP API, scheduler and job runner.

use dalsegno_app::acquisition::Library;
use dalsegno_app::config::Config;
use dalsegno_store::jobs::JobRow;
use runner::{JobFuture, JobHandler, JobRunner};
use scheduler::{SWEEP, run_scheduler};
use std::sync::Arc;
use tokio::signal;
use tokio::sync::watch;

pub mod runner;
pub mod scheduler;

/// Runs the jobs of the service.
#[derive(Debug)]
struct ServiceJobs {
    library: Library,
}

impl JobHandler for ServiceJobs {
    fn run(&self, job: JobRow) -> JobFuture {
        let library = self.library.clone();
        Box::pin(async move {
            match job.job.kind.as_str() {
                SWEEP => library
                    .sweep()
                    .await
                    .map(drop)
                    .map_err(|error| error.to_string()),
                other => Err(format!("unknown job kind {other}")),
            }
        })
    }
}

/// Runs the service until it is told to stop.
pub async fn serve(config: &Config) -> anyhow::Result<()> {
    let library = Library::open(&config.data)?;
    let database = library.database();
    let runner = Arc::new(JobRunner::new(
        database.clone(),
        Arc::new(ServiceJobs { library }),
    ));
    let recovered = runner.recover().map_err(anyhow::Error::msg)?;
    tracing::info!(recovered, "service starting");
    let (stop, shutdown) = watch::channel(false);
    let jobs = tokio::spawn(runner.run(shutdown.clone()));
    let schedule = tokio::spawn(run_scheduler(database, shutdown));
    signal::ctrl_c().await?;
    tracing::info!("service stopping");
    stop.send(true)?;
    jobs.await?;
    schedule.await?;
    Ok(())
}

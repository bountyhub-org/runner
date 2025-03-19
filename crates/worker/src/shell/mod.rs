use self::execution_context::ExecutionContext;
use super::Worker;
use client::invoker::{
    Client as InvokerClient, JobAcquiredResponse, WorkerRequestEvent, WorkerResponseEvent,
};
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use std::{path::Path, sync::Arc};
use steps::StepsRunner;
use tokio_util::sync::CancellationToken;

pub mod execution_context;
mod step;
pub mod steps;

#[derive(Clone)]
pub struct ShellWorker {
    pub root_workdir: String,
    pub envs: Arc<Vec<(String, String)>>,
    pub client: InvokerClient,
    pub job: JobAcquiredResponse,
}

impl Worker for ShellWorker {
    #[tracing::instrument(skip(self, ct))]
    async fn run(self, ct: CancellationToken) -> Result<()> {
        tracing::info!("Connecting worker with the invoker service");

        let (tx, mut rx) = self
            .client
            .worker_connect()
            .await
            .wrap_err("Failed to worker connect")?;

        tx.send(WorkerRequestEvent::ResolveJob {
            id: self.job.id,
            token: self.job.token,
        })
        .await
        .into_diagnostic()
        .wrap_err("Failed to send resolve job");

        let job = match rx.recv().await {
            Some(WorkerResponseEvent::JobResolved(resolved)) => resolved,
            None => bail!("Operation cancelled"),
        };

        tracing::info!("Building execution context");
        let workdir = Path::new(&self.root_workdir)
            .join(job.cfg.id.to_string())
            .to_str()
            .unwrap()
            .to_string();

        let job_name = job.cfg.name.clone();
        tracing::info!("Received job {job_name}");
        let execution_context = ExecutionContext::new(workdir, self.envs.clone(), job.cfg);

        tracing::info!("Building steps");
        let mut steps = StepsRunner::new(execution_context, job.steps);
        tracing::debug!("Built steps: {:?}", steps);

        tracing::info!("Running job: {}", job_name);
        steps
            .run(ct.clone(), self.client.clone())
            .await
            .wrap_err("steps.run failed")
    }
}

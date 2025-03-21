use self::execution_context::ExecutionContext;
use super::Worker;
use client::job::JobClient;
use client::runner::JobAcquiredResponse;
use ctx::{Background, Ctx};
use miette::{Result, WrapErr};
use std::{path::Path, sync::Arc};
use steps::StepsRunner;

pub mod execution_context;
pub mod step;
pub mod steps;

#[derive(Clone)]
pub struct ShellWorker<C>
where
    C: JobClient,
{
    pub root_workdir: String,
    pub envs: Arc<Vec<(String, String)>>,
    pub client: C,
    pub job: JobAcquiredResponse,
}

impl<C> Worker for ShellWorker<C>
where
    C: JobClient,
{
    #[tracing::instrument(skip(self, ctx))]
    fn run(self, ctx: Ctx<Background>) -> Result<()> {
        tracing::info!("Resolving job {}", self.job.id);
        let job = self
            .client
            .resolve(ctx.clone())
            .wrap_err("failed to resolve job")?;
        tracing::info!("Resolved job: {:?}", job);

        tracing::info!("Building execution context");
        let workdir = Path::new(&self.root_workdir)
            .join(job.cfg.id.to_string())
            .to_str()
            .unwrap()
            .to_string();

        let job_name = job.cfg.name.clone();
        let execution_context = ExecutionContext::new(workdir, self.envs.clone(), job.cfg);

        tracing::info!("Building steps");
        let mut steps = StepsRunner::new(execution_context, job.steps);
        tracing::debug!("Built steps: {:?}", steps);

        tracing::info!("Running job: {}", job_name);
        steps
            .run(ctx.clone(), &self.client)
            .wrap_err("steps.run failed")
    }
}

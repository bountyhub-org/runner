use self::execution_context::ExecutionContext;
use super::error::WorkerError;
use super::Worker;
use client::runner::PollResponse;
use client::worker::WorkerClient;
use ctx::{Background, Ctx};
use error_stack::{Result, ResultExt};
use std::path::Path;
use steps::Steps;

pub mod execution_context;
pub mod steps;

#[derive(Clone)]
pub struct ShellWorker<C>
where
    C: WorkerClient,
{
    pub root_workdir: String,
    pub envs: Vec<(String, String)>,
    pub worker_client: C,
}

impl<C> Worker for ShellWorker<C>
where
    C: WorkerClient,
{
    #[tracing::instrument(skip(self, ctx))]
    fn run(mut self, ctx: Ctx<Background>, job: PollResponse) -> Result<(), WorkerError> {
        self.worker_client = self.worker_client.with_token(job.token);

        tracing::info!("Resolving job {}", job.id);
        let job = self
            .worker_client
            .resolve_job(ctx.clone())
            .change_context(WorkerError)
            .attach_printable("failed to resolve job")?;
        tracing::info!("Resolved job: {:?}", job);

        tracing::info!("Building execution context");
        let workdir = Path::new(&self.root_workdir)
            .join(job.cfg.id.to_string())
            .to_str()
            .unwrap()
            .to_string();

        let job_name = job.cfg.name.clone();
        let mut execution_context = ExecutionContext::new(workdir, self.envs.clone(), job.cfg);

        tracing::info!("Building steps");
        let mut steps = Steps::new(job.steps, job.uploads);
        tracing::debug!("Built steps: {:?}", steps);

        tracing::info!("Running job: {}", job_name);
        steps
            .run(ctx.clone(), &mut execution_context, &self.worker_client)
            .change_context(WorkerError)
            .attach_printable("steps.run failed")
    }
}

use self::execution_context::ExecutionContext;
use super::Worker;
use client::job::{JobClient, Step};
use client::runner::JobAcquiredResponse;
use ctx::{Background, Ctx};
use miette::{Result, WrapErr};
use std::{path::Path, sync::Arc};
use step::{CommandStep, SetupStep, Step as ShellStep, TeardownStep, UploadStep};

pub mod execution_context;
pub mod step;

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
        tracing::info!("Resolved job: {}", job.cfg.name);

        tracing::info!("Building execution context");
        let workdir = Path::new(&self.root_workdir)
            .join(job.cfg.id.to_string())
            .to_str()
            .unwrap()
            .to_string();

        let mut execution_context = ExecutionContext::new(workdir, self.envs.clone(), job.cfg);

        for (index, step) in job.steps.iter().enumerate() {
            let index = index as u32;
            let ctx = ctx.clone();
            let worker_client = self.client.clone();

            let result = match step {
                Step::Setup => {
                    let step = SetupStep {
                        index,
                        context: &execution_context,
                        worker_client,
                    };

                    step.run(ctx)
                }
                Step::Teardown => {
                    let step = TeardownStep {
                        index,
                        context: &execution_context,
                        worker_client,
                    };

                    step.run(ctx)
                }
                Step::Upload { uploads } => {
                    let step = UploadStep {
                        index,
                        context: &execution_context,
                        uploads,
                        worker_client,
                    };

                    step.run(ctx)
                }
                Step::Command {
                    cond,
                    run,
                    shell,
                    allow_failed,
                } => {
                    let step = CommandStep {
                        index,
                        context: &execution_context,
                        worker_client,
                        cond,
                        run,
                        shell,
                        allow_failed: *allow_failed,
                    };

                    step.run(ctx)
                }
            };

            execution_context.set_ok(result.wrap_err("Failed to run")?);
        }

        Ok(())
    }
}

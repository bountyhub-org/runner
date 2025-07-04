use self::execution_context::ExecutionContext;
use super::Worker;
use client::worker::{Step, WorkerClient};
use ctx::{Background, Ctx};
use miette::{Result, WrapErr};
use std::path::PathBuf;
use std::sync::Arc;
use step::{CommandStep, SetupStep, Step as ShellStep, TeardownStep, UploadStep};
use uuid::Uuid;

pub mod execution_context;
pub mod step;

#[derive(Clone)]
pub struct ShellWorker<C>
where
    C: WorkerClient,
{
    pub root_workdir: PathBuf,
    pub env: Arc<Vec<(String, String)>>,
    pub client: C,
    pub id: Uuid,
}

impl<C> Worker for ShellWorker<C>
where
    C: WorkerClient,
{
    #[tracing::instrument(skip(self, ctx))]
    fn run(self, ctx: Ctx<Background>) -> Result<()> {
        tracing::info!("Resolving job {}", self.id);
        let job = self
            .client
            .resolve(ctx.clone())
            .wrap_err("failed to resolve job")?;
        tracing::info!("Resolved job: {}", job.cfg.name);

        tracing::info!("Building execution context");
        let mut execution_context =
            ExecutionContext::new(self.root_workdir.clone(), self.env.clone(), job.cfg);

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
                    allow_failure,
                } => {
                    let step = CommandStep {
                        index,
                        context: &execution_context,
                        worker_client,
                        cond,
                        run,
                        shell,
                        allow_failure: *allow_failure,
                    };

                    step.run(ctx)
                }
            };

            execution_context.set_ok(result.wrap_err("Failed to run")?);
        }

        Ok(())
    }
}

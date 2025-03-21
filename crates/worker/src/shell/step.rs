use super::execution_context::ExecutionContext;
use client::job::{JobClient, TimelineRequest, TimelineRequestStepOutcome};
use client::job::{LogLine, TimelineRequestStepState};
use ctx::{Background, Ctx};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::fs;

pub trait Step {
    fn run(&self, ctx: Ctx<Background>) -> Result<bool>;
}

#[derive(Debug, Clone)]
pub struct SetupStep<'a, C>
where
    C: JobClient,
{
    index: u32,
    context: &'a ExecutionContext,
    worker_client: C,
}

impl<C> Step for SetupStep<'_, C>
where
    C: JobClient,
{
    #[tracing::instrument(skip(self, ctx))]
    fn run(&self, ctx: Ctx<Background>) -> Result<bool> {
        tracing::debug!("Executing setup step");
        let workdir = self.context.workdir();

        let timeline_request = TimelineRequest {
            index: self.index,
            state: TimelineRequestStepState::Running,
        };
        tracing::debug!("Setting setup step running: {timeline_request:?}");
        self.worker_client
            .post_step_timeline(ctx.clone(), &timeline_request)
            .wrap_err("Failed to post step timeline")?;

        tracing::debug!("Creating workdir '{workdir}'");
        match fs::create_dir_all(workdir) {
            Ok(_) => {
                let msg = format!("Sucessfully created workdir '{workdir}'");
                tracing::debug!("{msg}");
                self.worker_client
                    .send_job_logs(ctx.clone(), vec![LogLine::stdout(self.index, &msg)])
                    .wrap_err("Failed to send logs")?;

                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: TimelineRequestStepState::Succeeded,
                };
                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;
                tracing::debug!("Posted step state: {timeline_request:?}");

                tracing::debug!("Posted setup step");
                Ok(true)
            }
            Err(e) => {
                let msg = format!("Failed to create workdir '{workdir}': {e:?}");
                tracing::debug!("{msg}");
                self.worker_client
                    .send_job_logs(ctx.clone(), vec![LogLine::stderr(self.index, &msg)])
                    .wrap_err("Failed to send logs")?;

                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: TimelineRequestStepState::Failed {
                        outcome: TimelineRequestStepOutcome::Failed,
                    },
                };

                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;
                tracing::debug!("Posted step state: {timeline_request:?}");

                tracing::debug!("Posted setup step");
                Ok(false)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TeardownStep<'a, C>
where
    C: JobClient,
{
    index: u32,
    context: &'a ExecutionContext,
    worker_client: C,
}

impl<C> Step for TeardownStep<'_, C>
where
    C: JobClient,
{
    #[tracing::instrument(skip(self, ctx))]
    fn run(&self, ctx: Ctx<Background>) -> Result<bool> {
        tracing::debug!("Executing teardown step");
        let workdir = self.context.workdir();

        let timeline_request = TimelineRequest {
            index: self.index,
            state: TimelineRequestStepState::Running,
        };
        tracing::debug!("Setting setup step running: {timeline_request:?}");
        self.worker_client
            .post_step_timeline(ctx.clone(), &timeline_request)
            .wrap_err("Failed to post step timeline")?;
    }
}

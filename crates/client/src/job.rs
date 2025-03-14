use crate::{
    error::{ClientError, OperationError},
    pool::ClientPool,
};
use config::Config;
use ctx::{Background, Ctx};
use jobengine::{JobMeta, ProjectMeta, WorkflowMeta, WorkflowRevisionMeta};
use miette::{Context, Result};
#[cfg(feature = "mockall")]
use mockall::mock;
use recoil::{Interval, Recoil, State};
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::{
    collections::BTreeMap,
    fmt,
    fs::File,
    sync::{Arc, RwLock},
    time::Duration,
};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobResolvedResponse {
    pub steps: Vec<Step>,
    pub cfg: jobengine::Config,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Step {
    Setup,
    Teardown,
    Upload {
        uploads: Vec<String>,
    },
    Command {
        cond: String,
        run: String,
        shell: String,
        allow_failed: bool,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobExecutionContext {
    pub scans: BTreeMap<String, Vec<JobMeta>>,
    pub vars: BTreeMap<String, String>,
    pub project: ProjectMeta,
    pub workflow: WorkflowMeta,
    pub revision: WorkflowRevisionMeta,
}

#[derive(Serialize, Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobContext {
    pub id: Uuid,
    pub state: String,
    pub nonce: Option<String>,
}

#[derive(Serialize, Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContext {
    pub id: Uuid,
}

#[derive(Serialize, Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowContext {
    pub id: Uuid,
}

#[derive(Serialize, Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RevisionContext {
    pub id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineRequest {
    pub index: u32,
    pub state: TimelineRequestStepState,
}

#[derive(Debug, Serialize, Copy, Clone)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "state", content = "outcome")]
pub enum TimelineRequestStepState {
    Running,
    Succeeded,
    Failed { outcome: TimelineRequestStepOutcome },
    Cancelled,
    Skipped,
}

impl fmt::Display for TimelineRequestStepState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimelineRequestStepState::Running => write!(f, "running"),
            TimelineRequestStepState::Succeeded => write!(f, "succeeded"),
            TimelineRequestStepState::Failed { outcome } => {
                write!(f, "failed (outcome = {})", outcome)
            }
            TimelineRequestStepState::Cancelled => write!(f, "cancelled"),
            TimelineRequestStepState::Skipped => write!(f, "skipped"),
        }
    }
}

impl TimelineRequestStepState {
    pub fn is_done(&self) -> bool {
        matches!(
            self,
            TimelineRequestStepState::Succeeded
                | TimelineRequestStepState::Failed { .. }
                | TimelineRequestStepState::Cancelled
                | TimelineRequestStepState::Skipped
        )
    }
}

#[derive(Debug, Serialize, Copy, Clone)]
#[serde(rename_all = "camelCase")]
pub enum TimelineRequestStepOutcome {
    Succeeded,
    Failed,
    Skipped,
}

impl fmt::Display for TimelineRequestStepOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimelineRequestStepOutcome::Succeeded => write!(f, "succeeded"),
            TimelineRequestStepOutcome::Failed => write!(f, "failed"),
            TimelineRequestStepOutcome::Skipped => write!(f, "skipped"),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct JobUploadRequest {
    size: u64,
}

#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr)]
#[repr(u8)]
pub enum LogDestination {
    Stdout = 1,
    Stderr = 2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub dst: LogDestination,
    pub step_index: u32,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    pub line: String,
}

pub trait JobClient: Send + Sync + Clone + 'static {
    fn resolve(&self, ctx: Ctx<Background>) -> Result<JobResolvedResponse>;
    fn post_step_timeline(&self, ctx: Ctx<Background>, timeline: &TimelineRequest) -> Result<()>;
    fn send_job_logs(&self, ctx: Ctx<Background>, logs: Vec<LogLine>) -> Result<()>;
    fn upload_job_artifact(&self, ctx: Ctx<Background>, file: File) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct StepRef {
    pub project_id: Uuid,
    pub workflow_id: Uuid,
    pub revision_id: Uuid,
    pub job_id: Uuid,
    pub step_index: u32,
}

#[cfg(feature = "mockall")]
mock! {
    pub JobClient {}

    impl Clone for JobClient {
        fn clone(&self) -> Self;
    }

    impl JobClient for JobClient {
        fn resolve(&self, ctx: Ctx<Background>) -> Result<JobResolvedResponse>;
        fn post_step_timeline(&self, ctx: Ctx<Background>, timeline: &TimelineRequest) -> Result<()>;

    fn send_job_logs(
        &self,
        ctx: Ctx<Background>,
        logs: Vec<LogLine>,
    ) -> Result<()>;
        fn upload_job_artifact(&self, ctx: Ctx<Background>, file: File) -> Result<()>;
    }

}

#[derive(Debug, Clone)]
pub struct HttpJobClient {
    token: String,
    user_agent: Arc<String>,
    recoil: Recoil,
    pool: ClientPool,
    config: Arc<RwLock<Config>>,
}

impl HttpJobClient {
    #[tracing::instrument]
    pub fn new(
        config: Arc<RwLock<Config>>,
        pool: ClientPool,
        token: &str,
        user_agent: Arc<String>,
    ) -> Self {
        Self {
            config,
            pool,
            token: format!("RunnerWorker {}", token),
            user_agent,
            recoil: Recoil {
                interval: Interval {
                    duration: Duration::from_secs(1),
                    multiplier: 2.0,
                    max_duration: None,
                    jitter: Some((0.9, 1.1)),
                },
                max_retries: Some(8),
            },
        }
    }
}

impl JobClient for HttpJobClient {
    #[tracing::instrument(skip(self, ctx))]
    fn resolve(&self, ctx: Ctx<Background>) -> Result<JobResolvedResponse> {
        let endpoint = {
            let config = self.config.read().unwrap();
            format!("{}/api/v0/jobs/resolve", config.invoker_url)
        };
        let client = self.pool.default_client();
        let retry = || ctx.is_done();
        let mut recoil = self.recoil.clone();
        let res = recoil.run(|| {
            if ctx.is_done() {
                return State::Fail(
                    miette::miette!("Context is cancelled").wrap_err(ClientError::FatalError),
                );
            }

            match client
                .post(&endpoint)
                .set("Authorization", &self.token)
                .set("User-Agent", &self.user_agent)
                .set("Content-Type", "application/json")
                .call()
                .map_err(ClientError::from)
            {
                Ok(res) => State::Done(res),
                Err(ClientError::RetryableError) => State::Retry(retry),
                Err(e) => State::Fail(miette::miette!("Failed to resolve the job").wrap_err(e)),
            }
        });

        match res {
            Ok(res) => {
                let res = res
                    .into_json::<JobResolvedResponse>()
                    .map_err(OperationError::from)
                    .wrap_err("Failed to deserialize job resolved response")?;

                Ok(res)
            }
            Err(recoil::recoil::Error::MaxRetriesReached) => {
                Err(OperationError::MaxRetriesError.into())
            }
            Err(recoil::recoil::Error::Custom(e)) => Err(e),
        }
    }

    #[tracing::instrument(skip(self, ctx))]
    fn post_step_timeline(&self, ctx: Ctx<Background>, timeline: &TimelineRequest) -> Result<()> {
        let endpoint = {
            let config = self.config.read().unwrap();
            format!("{}/api/v0/jobs/timeline", config.invoker_url)
        };
        let client = self.pool.default_client();
        let retry = || ctx.is_done();
        let mut recoil = self.recoil.clone();

        let res = recoil.run(|| {
            if ctx.is_done() {
                return State::Fail(
                    miette::miette!("Context is cancelled").wrap_err(ClientError::FatalError),
                );
            }

            match client
                .post(&endpoint)
                .set("Authorization", &self.token)
                .set("User-Agent", &self.user_agent)
                .set("Content-Type", "application/json")
                .send_json(ureq::json!(timeline))
                .map_err(ClientError::from)
            {
                Ok(res) => State::Done(res),
                Err(ClientError::RetryableError) => State::Retry(retry),
                Err(e) => State::Fail(miette::miette!("Failed to post the timeline").wrap_err(e)),
            }
        });

        match res {
            Ok(_) => Ok(()),
            Err(recoil::recoil::Error::MaxRetriesReached) => {
                Err(OperationError::MaxRetriesError.into())
            }
            Err(recoil::recoil::Error::Custom(e)) => Err(e),
        }
    }

    fn send_job_logs(&self, ctx: Ctx<Background>, logs: Vec<LogLine>) -> Result<()> {
        let endpoint = {
            let config = self.config.read().unwrap();
            format!("{}/api/v0/jobs/logs", &config.fluxy_url,)
        };

        let client = self.pool.stream_client();

        let retry = || ctx.is_done();
        let mut recoil = self.recoil.clone();

        let res = recoil.run(|| {
            if ctx.is_done() {
                return State::Fail(
                    miette::miette!("Context is cancelled").wrap_err(ClientError::FatalError),
                );
            }

            match client
                .patch(&endpoint)
                .set("Authorization", &self.token)
                .set("Content-Type", "application/json")
                .send_json(&logs)
                .map_err(ClientError::from)
            {
                Ok(_) => State::Done(()),
                Err(ClientError::RetryableError) => State::Retry(retry),
                Err(e) => State::Fail(miette::miette!("Failed to resolve the job").wrap_err(e)),
            }
        });

        match res {
            Ok(_) => Ok(()),
            Err(recoil::recoil::Error::MaxRetriesReached) => {
                Err(OperationError::MaxRetriesError.into())
            }
            Err(recoil::recoil::Error::Custom(e)) => Err(e),
        }
    }

    #[tracing::instrument(skip(self, ctx))]
    fn upload_job_artifact(&self, ctx: Ctx<Background>, file: File) -> Result<()> {
        let endpoint = {
            let config = self.config.read().unwrap();
            format!("{}/api/v0/jobs/results", &config.fluxy_url)
        };
        let retry = || ctx.is_done();
        let mut recoil = self.recoil.clone();

        let file_size = file.metadata().map_err(OperationError::from)?.len();

        let client = self.pool.assets_client();
        let res = recoil.run(|| {
            tracing::info!("Uploading results to {}", endpoint);
            match client
                .put(&endpoint)
                .set("Authorization", &self.token)
                .set("Content-Length", &file_size.to_string())
                .set("Content-Type", "application/octet-stream")
                .send(&file)
                .map_err(ClientError::from)
            {
                Ok(_) => State::Done(()),
                Err(ClientError::RetryableError) => State::Retry(retry),
                Err(e) => State::Fail(miette::miette!("Failed to resolve the job").wrap_err(e)),
            }
        });

        match res {
            Ok(_) => Ok(()),
            Err(recoil::recoil::Error::MaxRetriesReached) => {
                Err(OperationError::MaxRetriesError.into())
            }
            Err(recoil::recoil::Error::Custom(e)) => Err(e),
        }
    }
}

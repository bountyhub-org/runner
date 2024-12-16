use super::error::ClientError;
use crate::error::RetryableError;
use cel_interpreter::objects::Value;
use ctx::{Background, Ctx};
use error_stack::{Report, Result, ResultExt};
use jobengine::{JobMeta, ProjectMeta, WorkflowMeta, WorkflowRevisionMeta};
#[cfg(feature = "mockall")]
use mockall::mock;
use pipe::PipeReader;
use recoil::{Interval, Recoil, State};
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::{collections::BTreeMap, fmt, fs::File, time::Duration};
use ureq::Error as UreqError;
use uuid::Uuid;

pub const DEFAULT_INVOKER_URL: &str = "https://invoker.bountyhub.org";
pub const DEFAULT_FLUXY_URL: &str = "https://fluxy.bountyhub.org";

#[derive(Deserialize, Debug, Clone)]
pub struct JobResolvedResponse {
    pub steps: Vec<JobResolvedStepResponse>,
    pub uploads: Option<Vec<String>>,
    pub cfg: jobengine::Config,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize_repr, Deserialize_repr, Copy)]
#[repr(u8)]
pub enum StepKind {
    Setup,
    Teardown,
    Upload,
    Command,
}

#[derive(Deserialize, Debug, Clone)]
pub struct JobResolvedStepResponse {
    pub id: Uuid,
    pub kind: StepKind,
    pub run: String,
    pub shell: String,
    pub cond: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JobExecutionContext {
    pub scans: BTreeMap<String, Vec<JobMeta>>,
    pub vars: BTreeMap<String, String>,
    pub project: ProjectMeta,
    pub workflow: WorkflowMeta,
    pub revision: WorkflowRevisionMeta,
}

#[derive(Serialize, Debug, Deserialize, Clone)]
pub struct JobContext {
    pub id: Uuid,
    pub state: String,
    pub nonce: Option<String>,
}

impl TryFrom<Value> for JobContext {
    type Error = String;
    #[tracing::instrument]
    fn try_from(value: Value) -> std::result::Result<Self, Self::Error> {
        serde_json::from_value(value.json().map_err(|e| e.to_string())?).map_err(|e| e.to_string())
    }
}

#[derive(Serialize, Debug, Deserialize, Clone)]
pub struct ProjectContext {
    pub id: Uuid,
}

#[derive(Serialize, Debug, Deserialize, Clone)]
pub struct WorkflowContext {
    pub id: Uuid,
}

#[derive(Serialize, Debug, Deserialize, Clone)]
pub struct RevisionContext {
    pub id: Uuid,
}

#[derive(Debug, Serialize)]
pub struct TimelineRequest {
    pub id: Uuid,
    pub state: TimelineRequestStepState,
}

#[derive(Debug, Serialize, Copy, Clone)]
#[serde(rename_all = "lowercase")]
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

pub trait WorkerClient: Send + Sync + Clone + 'static {
    fn with_invoker_url(self, url: String) -> Self;
    fn with_fluxy_url(self, fluxy_url: String) -> Self;
    fn with_token(self, token: String) -> Self;
    fn resolve_job(&self, ctx: Ctx<Background>) -> Result<JobResolvedResponse, ClientError>;
    fn post_step_timeline(
        &self,
        ctx: Ctx<Background>,
        timeline: &TimelineRequest,
    ) -> Result<(), ClientError>;
    fn stream_job_step_log(
        &self,
        ctx: Ctx<Background>,
        project_id: Uuid,
        workflow_id: Uuid,
        revision_id: Uuid,
        job_id: Uuid,
        step_id: Uuid,
        reader: &mut PipeReader,
    ) -> Result<(), ClientError>;
    fn upload_job_artifact(
        &self,
        ctx: Ctx<Background>,
        project_id: Uuid,
        workflow_id: Uuid,
        revision_id: Uuid,
        job_id: Uuid,
        file: File,
    ) -> Result<(), ClientError>;
}

#[cfg(feature = "mockall")]
mock! {
    pub WorkerClient {}

    impl Clone for WorkerClient {
        fn clone(&self) -> Self;
    }

    impl WorkerClient for WorkerClient {
        fn with_invoker_url(self, url: String) -> Self;
        fn with_fluxy_url(self, fluxy_url: String) -> Self;
        fn with_token(self, token: String) -> Self;
        fn resolve_job(&self, ctx: Ctx<Background>) -> Result<JobResolvedResponse, ClientError>;
        fn post_step_timeline(&self, ctx: Ctx<Background>, timeline: &TimelineRequest) -> Result<(), ClientError>;
        fn stream_job_step_log(
            &self,
            ctx: Ctx<Background>,
            project_id: Uuid,
            workflow_id: Uuid,
            revision_id: Uuid,
            job_id: Uuid,
            step_id: Uuid,
            reader: &mut PipeReader,
        ) -> Result<(), ClientError>;
        fn upload_job_artifact(&self, ctx: Ctx<Background>, project_id: Uuid, workflow_id: Uuid, revision_id: Uuid, job_id: Uuid, file: File) -> Result<(), ClientError>;
    }

}

#[derive(Debug, Clone)]
pub struct WorkerHttpClient {
    invoker_url: String,
    fluxy_url: String,
    token: String,
    user_agent: String,
    agent: ureq::Agent,
    recoil: Recoil,
}

impl WorkerHttpClient {
    #[tracing::instrument]
    pub fn new(user_agent: String, agent: ureq::Agent) -> Self {
        Self {
            agent,
            invoker_url: DEFAULT_INVOKER_URL.to_string(),
            fluxy_url: DEFAULT_FLUXY_URL.to_string(),
            user_agent,
            token: String::new(),
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

impl WorkerClient for WorkerHttpClient {
    #[tracing::instrument(skip(self))]
    fn with_invoker_url(self, invoker_url: String) -> Self {
        Self {
            invoker_url,
            ..self
        }
    }

    #[tracing::instrument(skip(self))]
    fn with_fluxy_url(self, fluxy_url: String) -> Self {
        Self { fluxy_url, ..self }
    }

    #[tracing::instrument(skip(self, token))]
    fn with_token(self, token: String) -> Self {
        Self {
            token: format!("RunnerWorker {}", token),
            ..self
        }
    }

    #[tracing::instrument(skip(self, ctx))]
    fn resolve_job(&self, ctx: Ctx<Background>) -> Result<JobResolvedResponse, ClientError> {
        let endpoint = format!("{}/api/v0/jobs/resolve", self.invoker_url);
        let retry = || ctx.is_done();
        let mut recoil = self.recoil.clone();

        let res = recoil.run(|| {
            if ctx.is_done() {
                return State::Fail(Report::new(ClientError).attach_printable("cancelled"));
            }

            match self
                .agent
                .post(&endpoint)
                .set("Authorization", &self.token)
                .set("User-Agent", &self.user_agent)
                .set("Content-Type", "application/json")
                .call()
            {
                Ok(res) => State::Done(res),
                Err(UreqError::Transport(err))
                    if err.kind() == ureq::ErrorKind::ConnectionFailed =>
                {
                    State::Retry(retry)
                }
                Err(err) => State::Fail(
                    Report::new(ClientError)
                        .attach_printable(format!("Failed to resolve job: {:?}", err)),
                ),
            }
        });

        match res {
            Ok(res) => {
                let res = res
                    .into_json::<JobResolvedResponse>()
                    .change_context(ClientError)
                    .attach_printable("failed to read job resolved response")?;

                Ok(res)
            }
            Err(recoil::recoil::Error::MaxRetriesReached) => Err(Report::new(RetryableError)
                .attach_printable("Max retries reached")
                .change_context(ClientError)),
            Err(recoil::recoil::Error::Custom(e)) => Err(e),
        }
    }

    #[tracing::instrument(skip(self, ctx))]
    fn post_step_timeline(
        &self,
        ctx: Ctx<Background>,
        timeline: &TimelineRequest,
    ) -> Result<(), ClientError> {
        let endpoint = format!("{}/api/v0/jobs/timeline", self.invoker_url);
        let retry = || ctx.is_done();
        let mut recoil = self.recoil.clone();

        let res = recoil.run(|| {
            if ctx.is_done() {
                return State::Fail(Report::new(ClientError).attach_printable("cancelled"));
            }

            match self
                .agent
                .post(&endpoint)
                .set("Authorization", &self.token)
                .set("User-Agent", &self.user_agent)
                .set("Content-Type", "application/json")
                .send_json(ureq::json!(timeline))
            {
                Ok(res) => State::Done(res),
                Err(UreqError::Transport(err))
                    if err.kind() == ureq::ErrorKind::ConnectionFailed =>
                {
                    State::Retry(retry)
                }
                Err(err) => State::Fail(
                    Report::new(ClientError)
                        .attach_printable(format!("Failed to post step timeline: {:?}", err)),
                ),
            }
        });

        match res {
            Ok(_) => Ok(()),
            Err(recoil::recoil::Error::MaxRetriesReached) => Err(Report::new(RetryableError)
                .attach_printable("Max retries reached")
                .change_context(ClientError)),
            Err(recoil::recoil::Error::Custom(e)) => Err(e),
        }
    }

    #[tracing::instrument(skip(self, _ctx, reader))]
    fn stream_job_step_log(
        &self,
        _ctx: Ctx<Background>,
        project_id: Uuid,
        workflow_id: Uuid,
        revision_id: Uuid,
        job_id: Uuid,
        step_id: Uuid,
        reader: &mut PipeReader,
    ) -> Result<(), ClientError> {
        let url = format!(
            "{}/api/v0/invoker/projects/{}/workflows/{}/revisions/{}/jobs/{}/steps/{}/logs",
            &self.fluxy_url, project_id, workflow_id, revision_id, job_id, step_id
        );

        self.agent
            .put(&url)
            .set("Authorization", &self.token)
            .set("Content-Type", "application/octet-stream")
            .send(reader)
            .change_context(ClientError)
            .attach_printable("failed to stream logs")?;

        Ok(())
    }

    #[tracing::instrument(skip(self, ctx))]
    fn upload_job_artifact(
        &self,
        ctx: Ctx<Background>,
        project_id: Uuid,
        workflow_id: Uuid,
        revision_id: Uuid,
        job_id: Uuid,
        file: File,
    ) -> Result<(), ClientError> {
        let url = format!(
            "{}/api/v0/invoker/projects/{}/workflows/{}/revisions/{}/jobs/{}/results",
            &self.fluxy_url, project_id, workflow_id, revision_id, job_id
        );
        let retry = || ctx.is_done();
        let mut recoil = self.recoil.clone();

        let file_size = file
            .metadata()
            .attach_printable("failed to read metadata from zip file")
            .change_context(ClientError)?
            .len();

        let res = recoil.run(|| {
            tracing::info!("Uploading results to {}", url);
            match self
                .agent
                .put(&url)
                .set("Authorization", &self.token)
                .set("Content-Length", &file_size.to_string())
                .set("Content-Type", "application/octet-stream")
                .send(&file)
            {
                Ok(_) => State::Done(()),
                Err(UreqError::Transport(err))
                    if err.kind() == ureq::ErrorKind::ConnectionFailed =>
                {
                    State::Retry(retry)
                }
                Err(err) => State::Fail(
                    Report::new(ClientError)
                        .attach_printable(format!("Failed to upload job artifact: {:?}", err)),
                ),
            }
        });

        match res {
            Ok(_) => Ok(()),
            Err(recoil::recoil::Error::MaxRetriesReached) => {
                return Err(Report::new(RetryableError)
                    .attach_printable("Max retries reached")
                    .change_context(ClientError));
            }
            Err(recoil::recoil::Error::Custom(e)) => {
                return Err(e);
            }
        }
    }
}

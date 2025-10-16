use crate::error::ClientError;
use config::ConfigManager;
use ctx::{Background, Ctx};
use miette::Result;
#[cfg(feature = "mockall")]
use mockall::mock;
use recoil::{Recoil, State};
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::{fmt, fs::File, time::Duration};
use time::OffsetDateTime;

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
    Artifact {
        artifacts: Vec<WorkflowArtifact>,
    },
    Command {
        cond: String,
        run: String,
        shell: String,
        allow_failure: bool,
    },
}

#[derive(Debug, Deserialize, Serialize, Eq, PartialEq, Clone, Hash)]
#[serde(rename_all = "camelCase")]
/// Represents an artifact to be uploaded after the scan
pub struct WorkflowArtifact {
    /// The name of the artifact
    pub name: String,
    /// The paths glob pattern to match files for the artifact
    pub paths: Vec<String>,
    #[serde(rename = "if")]
    /// The condition to check before uploading the artifact
    pub cond: Option<String>,
    /// The time after which the artifact expires
    /// Useful to avoid filling up storage with old artifacts
    pub expires_in: Option<Duration>,
    #[serde(default)]
    /// If true, the artifact will not be tracked by the notification system
    pub untracked: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct UrlResponse {
    pub url: String,
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

impl LogLine {
    pub fn stdout(step_index: u32, line: &str) -> Self {
        Self {
            dst: LogDestination::Stdout,
            step_index,
            timestamp: OffsetDateTime::now_utc(),
            line: line.escape_default().to_string(),
        }
    }

    pub fn stderr(step_index: u32, line: &str) -> Self {
        Self {
            dst: LogDestination::Stderr,
            step_index,
            timestamp: OffsetDateTime::now_utc(),
            line: line.escape_default().to_string(),
        }
    }
}

pub trait WorkerClient: Send + Sync + Clone + 'static {
    fn resolve(&self, ctx: Ctx<Background>) -> Result<JobResolvedResponse>;
    fn post_step_timeline(&self, ctx: Ctx<Background>, timeline: &TimelineRequest) -> Result<()>;
    fn send_job_logs(&self, ctx: Ctx<Background>, logs: &[LogLine]) -> Result<()>;
    fn upload_job_artifact(&self, ctx: Ctx<Background>, name: &str, file: File) -> Result<()>;
}

#[cfg(feature = "mockall")]
mock! {
    pub WorkerClient {}

    impl Clone for WorkerClient {
        fn clone(&self) -> Self;
    }

    impl WorkerClient for WorkerClient {
        fn resolve(&self, ctx: Ctx<Background>) -> Result<JobResolvedResponse>;
        fn post_step_timeline(&self, ctx: Ctx<Background>, timeline: &TimelineRequest) -> Result<()>;
        fn send_job_logs(
            &self,
            ctx: Ctx<Background>,
            logs: &[LogLine],
        ) -> Result<()>;
        fn upload_job_artifact(&self, ctx: Ctx<Background>,name: &str, file: File) -> Result<()>;
    }
}

#[derive(Clone, Debug)]
pub struct HttpWorkerClient {
    pub recoil: Recoil,
    pub config_manager: ConfigManager,
    pub default_client: ureq::Agent,
    pub artifact_client: ureq::Agent,
    pub token: String,
}

impl WorkerClient for HttpWorkerClient {
    #[tracing::instrument(skip(self, ctx))]
    fn resolve(&self, ctx: Ctx<Background>) -> Result<JobResolvedResponse> {
        let (endpoint, token) = (
            format!(
                "{}/api/v0/jobs/resolve",
                self.config_manager.get()?.invoker_url
            ),
            format!("Bearer {}", self.token),
        );

        let retry = || ctx.is_done();
        let mut result = self
            .recoil
            .run(|| {
                if ctx.is_done() {
                    return State::Fail(ClientError::CancellationError);
                }

                match self
                    .default_client
                    .post(&endpoint)
                    .header("Authorization", &token)
                    .header("Content-Type", "application/json")
                    .send_empty()
                    .map_err(ClientError::from)
                {
                    Ok(res) => State::Done(res),
                    Err(e) if e.is_retryable() => {
                        tracing::error!("Encountered retryable error: {e:?}");
                        State::Retry(retry)
                    }
                    Err(e) => State::Fail(e),
                }
            })
            .map_err(ClientError::from)?;

        Ok(result
            .body_mut()
            .read_json::<JobResolvedResponse>()
            .map_err(ClientError::from)?)
    }

    #[tracing::instrument(skip(self, ctx))]
    fn post_step_timeline(&self, ctx: Ctx<Background>, timeline: &TimelineRequest) -> Result<()> {
        let (endpoint, token) = (
            format!(
                "{}/api/v0/jobs/timeline",
                self.config_manager.get()?.invoker_url
            ),
            format!("Bearer {}", self.token),
        );
        let retry = || ctx.is_done();

        self.recoil
            .run(|| {
                if ctx.is_done() {
                    return State::Fail(ClientError::CancellationError);
                }

                match self
                    .default_client
                    .post(&endpoint)
                    .header("Authorization", &token)
                    .send_json(timeline)
                    .map_err(ClientError::from)
                {
                    Ok(res) => State::Done(res),
                    Err(e) if e.is_retryable() => {
                        tracing::error!("Encountered retryable error: {e:?}");
                        State::Retry(retry)
                    }
                    Err(e) => State::Fail(e),
                }
            })
            .map_err(ClientError::from)?;

        Ok(())
    }

    fn send_job_logs(&self, ctx: Ctx<Background>, logs: &[LogLine]) -> Result<()> {
        let (endpoint, token) = (
            format!("{}/api/v0/jobs/logs", &self.config_manager.get()?.fluxy_url),
            format!("Bearer {}", self.token),
        );

        let retry = || ctx.is_done();

        self.recoil
            .run(|| {
                if ctx.is_done() {
                    return State::Fail(ClientError::CancellationError);
                }

                match self
                    .default_client
                    .patch(&endpoint)
                    .header("Authorization", &token)
                    .header("Content-Type", "application/json")
                    .send_json(logs)
                    .map_err(ClientError::from)
                {
                    Ok(res) => State::Done(res),
                    Err(e) if e.is_retryable() => {
                        tracing::error!("Encountered retryable error: {e:?}");
                        State::Retry(retry)
                    }
                    Err(e) => State::Fail(e),
                }
            })
            .map_err(ClientError::from)?;

        Ok(())
    }

    #[tracing::instrument(skip(self, ctx))]
    fn upload_job_artifact(&self, ctx: Ctx<Background>, name: &str, file: File) -> Result<()> {
        let (endpoint, token) = (
            format!(
                "{}/api/v0/jobs/artifact",
                &self.config_manager.get()?.invoker_url,
            ),
            format!("Bearer {}", self.token),
        );
        let retry = || ctx.is_done();

        let UrlResponse { url } = self
            .recoil
            .run(|| {
                tracing::info!("Fetching artifact URL for artifact '{name}'");
                match self
                    .default_client
                    .post(&endpoint)
                    .header("Authorization", &token)
                    .header("Content-Type", "application/json")
                    .send_json(serde_json::json!({"name": name}))
                    .map_err(ClientError::from)
                {
                    Ok(mut res) => {
                        match res
                            .body_mut()
                            .read_json::<UrlResponse>()
                            .map_err(ClientError::from)
                        {
                            Ok(res) => State::Done(res),
                            Err(e) => State::Fail(e),
                        }
                    }
                    Err(e) if e.is_retryable() => {
                        tracing::error!(
                            "Encountered retryable error while fetching the upload url: {e:?}"
                        );
                        State::Retry(retry)
                    }
                    Err(e) => State::Fail(e),
                }
            })
            .map_err(ClientError::from)?;

        self.recoil
            .run(|| {
                tracing::info!("Uploading results to {}", url);
                match self
                    .artifact_client
                    .put(&url)
                    .header("Authorization", &token)
                    .header("Content-Type", "application/octet-stream")
                    .send(&file)
                    .map_err(ClientError::from)
                {
                    Ok(_) => State::Done(()),
                    Err(e) if e.is_retryable() => {
                        tracing::error!(
                            "Encountered retryable error while trying to upload artifact: {e:?}"
                        );
                        State::Retry(retry)
                    }
                    Err(e) => State::Fail(e),
                }
            })
            .map_err(ClientError::from)?;

        Ok(())
    }
}

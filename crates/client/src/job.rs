use config::ConfigManager;
use ctx::{Background, Ctx};
use jobengine::{JobMeta, ProjectMeta, WorkflowMeta, WorkflowRevisionMeta};
use miette::{Context, Result};
#[cfg(feature = "mockall")]
use mockall::mock;
use recoil::{Interval, Recoil, State};
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::{collections::BTreeMap, fmt, fs::File, sync::Arc, time::Duration};
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

pub trait JobClient: Send + Sync + Clone + 'static {
    fn resolve(&self, ctx: Ctx<Background>) -> Result<JobResolvedResponse>;
    fn post_step_timeline(&self, ctx: Ctx<Background>, timeline: &TimelineRequest) -> Result<()>;
    fn send_job_logs(&self, ctx: Ctx<Background>, logs: &[LogLine]) -> Result<()>;
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
        logs: &[LogLine],
    ) -> Result<()>;
        fn upload_job_artifact(&self, ctx: Ctx<Background>, file: File) -> Result<()>;
    }

}

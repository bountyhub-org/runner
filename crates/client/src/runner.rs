use config::ConfigManager;
use ctx::{Background, Ctx};
use miette::{Result, WrapErr};
#[cfg(feature = "mockall")]
use mockall::mock;
use recoil::{Interval, Recoil, State};
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PollRequest {
    pub capacity: u32,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobAcquiredResponse {
    pub id: Uuid,
    #[serde(skip_serializing)]
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteRequest {
    pub job_id: Uuid,
}

pub trait RunnerClient: Send + Sync + Clone {
    fn hello(&self, ctx: Ctx<Background>) -> Result<()>;
    fn goodbye(&self, ctx: Ctx<Background>) -> Result<()>;
    fn request(&self, ctx: Ctx<Background>, req: &PollRequest) -> Result<Vec<JobAcquiredResponse>>;
    fn complete(&self, ctx: Ctx<Background>, req: &CompleteRequest) -> Result<()>;
}

#[cfg(feature = "mockall")]
mock! {
    pub RunnerClient {}

    impl Clone for RunnerClient {
        fn clone(&self) -> Self;
    }

    impl RunnerClient for RunnerClient {
        fn hello(&self, ctx: Ctx<Background>) -> Result<()>;
        fn goodbye(&self, ctx: Ctx<Background>) -> Result<()>;
        fn request(&self, ctx: Ctx<Background>, req: &PollRequest) -> Result<Vec<JobAcquiredResponse>>;
        fn complete(&self, ctx: Ctx<Background>, req: &CompleteRequest) -> Result<()>;
    }
}

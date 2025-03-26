use config::ConfigManager;
use ctx::{Background, Ctx};
use miette::Result;
#[cfg(feature = "mockall")]
use mockall::mock;
use recoil::{Recoil, State};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ClientError;

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

#[derive(Debug, Clone)]
pub struct HttpRunnerClient {
    pub recoil: Recoil,
    pub config_manager: ConfigManager,
    pub default_client: ureq::Agent,
    pub long_poll_client: ureq::Agent,
}

impl RunnerClient for HttpRunnerClient {
    #[tracing::instrument(skip(self, ctx))]
    fn hello(&self, ctx: Ctx<Background>) -> Result<()> {
        let (endpoint, token) = {
            let cfg = self.config_manager.get()?;

            (
                format!("{}/api/v0/runners/hello", cfg.invoker_url),
                format!("Runner {}", cfg.token),
            )
        };

        let retry = || ctx.is_done();
        self.recoil
            .run(|| {
                if ctx.is_done() {
                    return State::Fail(ClientError::CancellationError);
                }
                tracing::debug!("Saying hello to the server");
                match self
                    .default_client
                    .post(&endpoint)
                    .header("Authorization", &token)
                    .send_empty()
                    .map_err(ClientError::from)
                {
                    Ok(res) => State::Done(res),
                    Err(e) if e.is_retryable() => State::Retry(retry),
                    Err(e) => State::Fail(e),
                }
            })
            .map_err(ClientError::from)?;

        Ok(())
    }

    #[tracing::instrument(skip(self, ctx))]
    fn goodbye(&self, ctx: Ctx<Background>) -> Result<()> {
        let (endpoint, token) = {
            let cfg = self.config_manager.get()?;

            (
                format!("{}/api/v0/runners/goodbye", cfg.invoker_url),
                format!("Runner {}", cfg.token),
            )
        };
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
                    .send_empty()
                    .map_err(ClientError::from)
                {
                    Ok(res) => State::Done(res),
                    Err(e) if e.is_retryable() => State::Retry(retry),
                    Err(e) => State::Fail(e),
                }
            })
            .map_err(ClientError::from)?;

        Ok(())
    }

    #[tracing::instrument(skip(self, ctx))]
    fn request(&self, ctx: Ctx<Background>, req: &PollRequest) -> Result<Vec<JobAcquiredResponse>> {
        let (endpoint, token) = {
            let cfg = self.config_manager.get()?;
            (
                format!("{}/api/v0/jobs/request", cfg.invoker_url),
                format!("Runner {}", cfg.token),
            )
        };
        let retry = || ctx.is_done();

        let mut result = self
            .recoil
            .run(|| {
                if ctx.is_done() {
                    return State::Fail(ClientError::CancellationError);
                }
                match self
                    .long_poll_client
                    .post(&endpoint)
                    .header("Authorization", &token)
                    .header("Content-Type", "application/json")
                    .send_json(req)
                    .map_err(ClientError::from)
                {
                    Ok(res) => State::Done(res),
                    Err(e) if e.is_retryable() => State::Retry(retry),
                    Err(e) => State::Fail(e),
                }
            })
            .map_err(ClientError::from)?;

        Ok(result
            .body_mut()
            .read_json::<Vec<JobAcquiredResponse>>()
            .map_err(ClientError::from)?)
    }

    #[tracing::instrument(skip(self, ctx))]
    fn complete(&self, ctx: Ctx<Background>, req: &CompleteRequest) -> Result<()> {
        let (endpoint, token) = {
            let cfg = self.config_manager.get()?;
            (
                format!("{}/api/v0/jobs/complete", cfg.invoker_url),
                format!("Runner {}", cfg.token),
            )
        };
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
                    .header("Content-Type", "application/json")
                    .send_json(req)
                    .map_err(ClientError::from)
                {
                    Ok(res) => State::Done(res),
                    Err(e) if e.is_retryable() => State::Retry(retry),
                    Err(e) => State::Fail(e),
                }
            })
            .map_err(ClientError::from)?;

        Ok(())
    }
}

use super::error::{ClientError, OperationError};
use crate::pool::ClientPool;
use config::Config;
use ctx::{Background, Ctx};
use miette::{Result, WrapErr};
#[cfg(feature = "mockall")]
use mockall::mock;
use recoil::{Interval, Recoil, State};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, RwLock},
    time::Duration,
};
use ureq::Response;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct PollRequest {
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
struct CompleteRequest {
    pub job_id: Uuid,
}

pub trait RunnerClient: Send + Sync + Clone {
    fn hello(&self, ctx: Ctx<Background>) -> Result<()>;
    fn goodbye(&self, ctx: Ctx<Background>) -> Result<()>;
    fn request(&self, ctx: Ctx<Background>, capacity: u32) -> Result<Vec<JobAcquiredResponse>>;
    fn complete(&self, ctx: Ctx<Background>, id: Uuid) -> Result<()>;
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
        fn request(&self, ctx: Ctx<Background>, capacity: u32) -> Result<Vec<JobAcquiredResponse>>;
        fn complete(&self, ctx: Ctx<Background>, id: Uuid) -> Result<()>;
    }
}

#[derive(Debug, Clone)]
pub struct HttpRunnerClient {
    user_agent: Arc<String>,
    pool: ClientPool,
    recoil: Recoil,
    config: Arc<RwLock<Config>>,
}

impl HttpRunnerClient {
    #[tracing::instrument]
    pub fn new(config: Arc<RwLock<Config>>, pool: ClientPool, user_agent: Arc<String>) -> Self {
        Self {
            config,
            pool,
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

impl RunnerClient for HttpRunnerClient {
    #[tracing::instrument(skip(self, ctx))]
    fn hello(&self, ctx: Ctx<Background>) -> Result<()> {
        let (endpoint, token) = {
            let cfg = self.config.read().unwrap();

            (
                format!("{}/api/v0/runners/hello", cfg.invoker_url),
                format!("Runner {}", cfg.token),
            )
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
            tracing::debug!("Saying hello to the server");
            match client
                .post(&endpoint)
                .set("Authorization", &token)
                .set("User-Agent", &self.user_agent)
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
                self.update_token(&res);
                Ok(())
            }
            Err(recoil::recoil::Error::MaxRetriesReached) => {
                Err(OperationError::MaxRetriesError.into())
            }
            Err(recoil::recoil::Error::Custom(e)) => Err(e),
        }
    }

    #[tracing::instrument(skip(self, ctx))]
    fn goodbye(&self, ctx: Ctx<Background>) -> Result<()> {
        let (endpoint, token) = {
            let cfg = self.config.read().unwrap();

            (
                format!("{}/api/v0/runners/goodbye", cfg.invoker_url),
                format!("Runner {}", cfg.token),
            )
        };
        let retry = || ctx.is_done();
        let client = self.pool.default_client();
        let mut recoil = self.recoil.clone();
        let res = recoil.run(|| {
            if ctx.is_done() {
                return State::Fail(
                    miette::miette!("Context is cancelled").wrap_err(ClientError::FatalError),
                );
            }
            match client
                .post(&endpoint)
                .set("Authorization", &token)
                .set("User-Agent", &self.user_agent)
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
                self.update_token(&res);
                Ok(())
            }
            Err(recoil::recoil::Error::MaxRetriesReached) => {
                Err(OperationError::MaxRetriesError.into())
            }
            Err(recoil::recoil::Error::Custom(e)) => Err(e),
        }
    }

    #[tracing::instrument(skip(self, ctx))]
    fn request(&self, ctx: Ctx<Background>, capacity: u32) -> Result<Vec<JobAcquiredResponse>> {
        let (endpoint, token) = {
            let cfg = self.config.read().unwrap();
            (
                format!("{}/api/v0/jobs/request", cfg.invoker_url),
                format!("Runner {}", cfg.token),
            )
        };
        let retry = || ctx.is_done();
        let mut recoil = self.recoil.clone();

        let client = self.pool.long_poll_client();
        let res = recoil.run(|| {
            if ctx.is_done() {
                return State::Fail(
                    miette::miette!("Context is cancelled").wrap_err(ClientError::FatalError),
                );
            }
            match client
                .post(&endpoint)
                .set("Authorization", &token)
                .set("User-Agent", &self.user_agent)
                .set("Content-Type", "application/json")
                .send_json(ureq::json!(PollRequest { capacity }))
                .map_err(ClientError::from)
            {
                Ok(res) => State::Done(res),
                Err(ClientError::RetryableError) => State::Retry(retry),
                Err(e) => State::Fail(miette::miette!("Failed to resolve the job").wrap_err(e)),
            }
        });

        match res {
            Ok(res) => {
                self.update_token(&res);
                let res: Vec<JobAcquiredResponse> = res
                    .into_json()
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
    fn complete(&self, ctx: Ctx<Background>, job_id: Uuid) -> Result<()> {
        let (endpoint, token) = {
            let cfg = self.config.read().unwrap();
            (
                format!("{}/api/v0/jobs/complete", cfg.invoker_url),
                format!("Runner {}", cfg.token),
            )
        };
        let retry = || ctx.is_done();
        let mut recoil = self.recoil.clone();

        let client = self.pool.default_client();
        let res = recoil.run(|| {
            if ctx.is_done() {
                return State::Fail(
                    miette::miette!("Context is cancelled").wrap_err(ClientError::FatalError),
                );
            }

            match client
                .post(&endpoint)
                .set("Authorization", &token)
                .set("User-Agent", &self.user_agent)
                .set("Content-Type", "application/json")
                .send_json(ureq::json!(CompleteRequest { job_id }))
                .map_err(ClientError::from)
            {
                Ok(res) => State::Done(res),
                Err(ClientError::RetryableError) => State::Retry(retry),
                Err(e) => State::Fail(miette::miette!("Failed to resolve the job").wrap_err(e)),
            }
        });

        match res {
            Ok(res) => {
                self.update_token(&res);
                Ok(())
            }
            Err(recoil::recoil::Error::MaxRetriesReached) => {
                Err(OperationError::MaxRetriesError.into())
            }
            Err(recoil::recoil::Error::Custom(e)) => Err(e),
        }
    }
}

impl HttpRunnerClient {
    #[tracing::instrument]
    fn update_token(&self, resp: &Response) {
        if let Some(token) = resp.header("X-Authorization-Refresh") {
            let mut cfg = self.config.write().unwrap();
            cfg.token = token.to_string();
            cfg.write().unwrap();
        };
    }
}

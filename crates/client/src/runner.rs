use super::error::{ClientError, OperationError};
use crate::pool::Client;
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
    pool: Client,
    recoil: Recoil,
    config_manager: ConfigManager,
}

impl HttpRunnerClient {
    #[tracing::instrument]
    pub fn new(config_manager: ConfigManager, pool: Client, user_agent: Arc<String>) -> Self {
        Self {
            config_manager,
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
            let cfg = self.config_manager.get()?;

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
                return State::Fail(ClientError::CancellationError.into());
            }
            tracing::debug!("Saying hello to the server");
            match client
                .post(&endpoint)
                .header("Authorization", &token)
                .send_empty()
                .map_err(ClientError::from)
            {
                Ok(res) => State::Done(res),
                Err(ClientError::ServerError | ClientError::ConnectionError(..)) => {
                    State::Retry(retry)
                }
                Err(e) => State::Fail(
                    miette::miette!("Failed to initialize contact with the server").wrap_err(e),
                ),
            }
        });

        match res {
            Ok(res) => self.update_token_if_refreshed(&res),
            Err(recoil::recoil::Error::MaxRetriesReachedError) => {
                Err(OperationError::MaxRetriesError.into())
            }
            Err(recoil::recoil::Error::UserError(e)) => Err(e),
        }
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
        let client = self.pool.default_client();
        let mut recoil = self.recoil.clone();
        let res = recoil.run(|| {
            if ctx.is_done() {
                return State::Fail(ClientError::CancellationError.into());
            }
            match client
                .post(&endpoint)
                .header("Authorization", &token)
                .header("User-Agent", &self.user_agent)
                .send_empty()
                .map_err(ClientError::from)
            {
                Ok(res) => State::Done(res),
                Err(ClientError::ServerError | ClientError::ConnectionError(..)) => {
                    State::Retry(retry)
                }
                Err(e) => State::Fail(miette::miette!("Failed to notify shutdown").wrap_err(e)),
            }
        });

        match res {
            Ok(res) => self.update_token_if_refreshed(&res),
            Err(recoil::recoil::Error::MaxRetriesReachedError) => {
                Err(OperationError::MaxRetriesError.into())
            }
            Err(recoil::recoil::Error::UserError(e)) => Err(e),
        }
    }

    #[tracing::instrument(skip(self, ctx))]
    fn request(&self, ctx: Ctx<Background>, capacity: u32) -> Result<Vec<JobAcquiredResponse>> {
        let (endpoint, token) = {
            let cfg = self.config_manager.get()?;
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
                return State::Fail(ClientError::CancellationError.into());
            }
            match client
                .post(&endpoint)
                .header("Authorization", &token)
                .header("User-Agent", &self.user_agent)
                .header("Content-Type", "application/json")
                .send_json(PollRequest { capacity })
                .map_err(ClientError::from)
            {
                Ok(res) => State::Done(res),
                Err(ClientError::ServerError | ClientError::ConnectionError(..)) => {
                    State::Retry(retry)
                }
                Err(e) => State::Fail(miette::miette!("Failed to request the job").wrap_err(e)),
            }
        });

        match res {
            Ok(res) => {
                self.update_token_if_refreshed(&res)?;
                let res: Vec<JobAcquiredResponse> =
                    res.read_json()
                        .map_err(OperationError::from)
                        .wrap_err("Failed to deserialize job resolved response")?;
                Ok(res)
            }
            Err(recoil::recoil::Error::MaxRetriesReachedError) => {
                Err(OperationError::MaxRetriesError.into())
            }
            Err(recoil::recoil::Error::UserError(e)) => Err(e),
        }
    }

    #[tracing::instrument(skip(self, ctx))]
    fn complete(&self, ctx: Ctx<Background>, job_id: Uuid) -> Result<()> {
        let (endpoint, token) = {
            let cfg = self.config_manager.get()?;
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
                return State::Fail(ClientError::CancellationError.into());
            }

            match client
                .post(&endpoint)
                .header("Authorization", &token)
                .header("User-Agent", &self.user_agent)
                .header("Content-Type", "application/json")
                .send_json(CompleteRequest { job_id })
                .map_err(ClientError::from)
            {
                Ok(res) => State::Done(res),
                Err(ClientError::ServerError | ClientError::ConnectionError(..)) => {
                    State::Retry(retry)
                }
                Err(e) => State::Fail(miette::miette!("Failed to complete the job").wrap_err(e)),
            }
        });

        match res {
            Ok(res) => self.update_token_if_refreshed(&res),
            Err(recoil::recoil::Error::MaxRetriesReachedError) => {
                Err(OperationError::MaxRetriesError.into())
            }
            Err(recoil::recoil::Error::UserError(e)) => Err(e),
        }
    }
}

impl HttpRunnerClient {
    #[tracing::instrument]
    #[inline]
    fn update_token_if_refreshed(&self, resp: &Response) -> Result<()> {
        if let Some(token) = resp.header("X-Authorization-Refresh") {
            let mut cfg = self.config_manager.get()?;
            cfg.token = token.to_string();
            self.config_manager.put(&cfg)?;
        };
        Ok(())
    }
}

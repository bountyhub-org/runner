use config::ConfigManager;
use ctx::{Background, Ctx};
use miette::Result;
#[cfg(feature = "mockall")]
use recoil::{Interval, Recoil, State};
use std::{sync::Arc, time::Duration};
use ureq::{middleware::Middleware, RequestBuilder};

use crate::{error::ClientError, runner::RunnerClient};

#[derive(Debug, Clone)]
pub struct ClientSet {
    inner: Arc<InnerClientSet>,
}

#[derive(Debug, Clone)]
struct InnerClientSet {
    /// The client used for long-polling requests.
    /// This client has a longer read timeout than the default client.
    /// The long poll is around 1 minute. This client by default
    /// times out after double the long poll time.
    long_poll_client: ureq::Agent,

    /// The default client is client that should be used for
    /// almost all requests. Unless the request is issued for
    /// special purpose endpoint, this client should be used.
    default_client: ureq::Agent,

    /// The client used for assets requests.
    /// This client has a longer read timeout than the default client.
    assets_client: ureq::Agent,

    /// The client used for stream requests. This is currently used
    /// for log streaming.
    stream_client: ureq::Agent,

    // Used to retry requests
    recoil: Recoil,

    config_manager: ConfigManager,
}

impl InnerClientSet {
    fn new(cfg: ClientSetConfig) -> Self {
        Self {
            long_poll_client: cfg.long_poll_client.agent(),
            assets_client: cfg.assets_client.agent(),
            default_client: cfg.default_client.agent(),
            stream_client: cfg.stream_client.agent(),
            recoil: cfg.recoil,
            config_manager: cfg.config_manager,
        }
    }
}

impl ClientSet {
    pub fn new(cfg: ClientSetConfig) -> Self {
        Self {
            inner: Arc::new(InnerClientSet::new(cfg)),
        }
    }

    pub fn long_poll_client(&self) -> ureq::Agent {
        self.inner.long_poll_client.clone()
    }

    pub fn default_client(&self) -> ureq::Agent {
        self.inner.default_client.clone()
    }

    pub fn assets_client(&self) -> ureq::Agent {
        self.inner.assets_client.clone()
    }

    pub fn stream_client(&self) -> ureq::Agent {
        self.inner.stream_client.clone()
    }
}

impl Default for ClientSet {
    fn default() -> Self {
        let user_agent = format!("runner/{} (cli)", env!("CARGO_PKG_VERSION"));
        Self::new(ClientSetConfig {
            default_client: ClientConfig {
                user_agent: user_agent.clone(),
                timeout_connect: Duration::from_secs(10),
                timeout_read: Duration::from_secs(30),
                timeout_write: Duration::from_secs(30),
            },
            long_poll_client: ClientConfig {
                user_agent: user_agent.clone(),
                timeout_connect: Duration::from_secs(10),
                timeout_read: Duration::from_secs(2 * 60),
                timeout_write: Duration::from_secs(10),
            },
            assets_client: ClientConfig {
                user_agent: user_agent.clone(),
                timeout_connect: Duration::from_secs(10),
                timeout_read: Duration::from_secs(2 * 60),
                timeout_write: Duration::from_secs(2 * 60),
            },
            stream_client: ClientConfig {
                user_agent: user_agent.clone(),
                timeout_connect: Duration::from_secs(10),
                timeout_read: Duration::from_secs(45 * 60),
                timeout_write: Duration::from_secs(45 * 60),
            },
            recoil: Recoil::default(),
            config_manager: ConfigManager::default(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub user_agent: String,
    pub timeout_connect: Duration,
    pub timeout_read: Duration,
    pub timeout_write: Duration,
}

impl ClientConfig {
    fn agent(&self) -> ureq::Agent {
        let cfg = ureq::Agent::config_builder()
            .timeout_connect(Some(self.timeout_connect))
            .timeout_send_request(Some(self.timeout_write))
            .timeout_recv_response(Some(self.timeout_read))
            .user_agent(&self.user_agent)
            .build();

        ureq::Agent::new_with_config(cfg)
    }
}

#[derive(Debug, Clone)]
pub struct ClientSetConfig {
    pub default_client: ClientConfig,
    pub long_poll_client: ClientConfig,
    pub assets_client: ClientConfig,
    pub stream_client: ClientConfig,
    pub recoil: Recoil,
    pub config_manager: ConfigManager,
}

impl RunnerClient for ClientSet {
    #[tracing::instrument(skip(self, ctx))]
    fn hello(&self, ctx: Ctx<Background>) -> Result<()> {
        let (endpoint, token) = {
            let cfg = self.inner.config_manager.get()?;

            (
                format!("{}/api/v0/runners/hello", cfg.invoker_url),
                format!("Runner {}", cfg.token),
            )
        };

        let client = self.inner.default_client;
        let retry = || ctx.is_done();
        self.inner
            .recoil
            .run(|| {
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
            })
            .map_err()
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

#[derive(Debug, Clone)]
struct TokenRefreshMiddleware {
    config_manager: ConfigManager,
}

impl Middleware for TokenRefreshMiddleware {
    #[tracing::instrument(skip(self, next, request))]
    fn handle(
        &self,
        request: ureq::http::Request<ureq::SendBody>,
        next: ureq::middleware::MiddlewareNext,
    ) -> std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error> {
        let response = next.handle(request)?;
        let token = match response.headers().get("X-Authorization-Refresh") {
            Some(t) => t,
            None => return Ok(response),
        };

        let token: &str = match token.to_str() {
            Ok(token) => token,
            Err(e) => {
                tracing::error!("Refresh authorization failed: {e:?}; continuing...");
                return Ok(response);
            }
        };

        let mut cfg = self.config_manager.get().expect("Get config to succeed");
        cfg.token = token.to_string();
        self.config_manager
            .put(&cfg)
            .expect("Should store configuration");

        Ok(response)
    }
}

use super::error::{ClientError, FatalError, RetryableError};
use ctx::{Background, Ctx};
use error_stack::{Report, Result, ResultExt};
#[cfg(feature = "mockall")]
use mockall::mock;
use recoil::{Interval, Recoil, State};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use ureq::Error as UreqError;
use uuid::Uuid;

pub const DEFAULT_INVOKER_URL: &str = "https://invoker.bountyhub.org";

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct PollRequest {
    pub capacity: u32,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PollResponse {
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
    fn with_token(self, id: String) -> Self;
    fn with_url(self, url: String) -> Self;
    fn hello(&self, ctx: Ctx<Background>) -> Result<(), ClientError>;
    fn goodbye(&self, ctx: Ctx<Background>) -> Result<(), ClientError>;
    fn request(
        &self,
        ctx: Ctx<Background>,
        capacity: u32,
    ) -> Result<Vec<PollResponse>, ClientError>;
    fn complete(&self, ctx: Ctx<Background>, id: Uuid) -> Result<(), ClientError>;
}

#[cfg(feature = "mockall")]
mock! {
    pub RunnerClient {}

    impl Clone for RunnerClient {
        fn clone(&self) -> Self;
    }

    impl RunnerClient for RunnerClient {
        fn with_token(self, id: String) -> Self;
        fn with_url(self, url: String) -> Self;
        fn hello(&self, ctx: Ctx<Background>) -> Result<(), ClientError>;
        fn goodbye(&self, ctx: Ctx<Background>) -> Result<(), ClientError>;
        fn request(&self, ctx: Ctx<Background>, capacity: u32) -> Result<Vec<PollResponse>, ClientError>;
        fn complete(&self, ctx: Ctx<Background>, id: Uuid) -> Result<(), ClientError>;
    }
}

#[derive(Debug, Clone)]
pub struct RunnerHttpClient {
    url: String,
    token: String,
    user_agent: String,
    agent: ureq::Agent,
    recoil: Recoil,
}

impl RunnerHttpClient {
    #[tracing::instrument]
    pub fn new(user_agent: String, agent: ureq::Agent) -> Self {
        Self {
            agent,
            url: DEFAULT_INVOKER_URL.to_string(),
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

impl RunnerClient for RunnerHttpClient {
    #[tracing::instrument(skip(token))]
    fn with_token(self, token: String) -> Self {
        Self {
            token: format!("Runner {}", token),
            ..self
        }
    }

    #[tracing::instrument]
    fn with_url(self, url: String) -> Self {
        Self { url, ..self }
    }

    #[tracing::instrument(skip(self, ctx))]
    fn hello(&self, ctx: Ctx<Background>) -> Result<(), ClientError> {
        let endpoint = format!("{}/api/v0/runners/hello", self.url);
        let retry = || ctx.is_done();
        let mut recoil = self.recoil.clone();

        let res = recoil.run(|| {
            if ctx.is_done() {
                tracing::debug!("Cancelled");
                return State::Fail(Report::new(ClientError).attach_printable("cancelled"));
            }
            tracing::debug!("Saying hello to the server");
            match self
                .agent
                .post(&endpoint)
                .set("Authorization", &self.token)
                .set("User-Agent", &self.user_agent)
                .call()
            {
                Ok(res) => State::Done(res),
                Err(UreqError::Status(403, ..)) => State::Fail(
                    Report::new(FatalError)
                        .attach_printable("Unauthorized")
                        .change_context(ClientError),
                ),
                Err(UreqError::Status((500..), ..)) => State::Retry(retry),
                Err(UreqError::Transport(err))
                    if err.kind() == ureq::ErrorKind::ConnectionFailed =>
                {
                    State::Retry(retry)
                }
                Err(err) => State::Fail(
                    Report::new(ClientError)
                        .attach_printable(format!("Failed to say hello: {}", err)),
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

    #[tracing::instrument(skip(self, ctx))]
    fn goodbye(&self, ctx: Ctx<Background>) -> Result<(), ClientError> {
        let endpoint = format!("{}/api/v0/runners/goodbye", self.url);
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
                .call()
            {
                Ok(res) => State::Done(res),
                Err(UreqError::Status(403, ..)) => State::Fail(
                    Report::new(FatalError)
                        .attach_printable("Unauthorized")
                        .change_context(ClientError),
                ),
                Err(UreqError::Status((500..), ..)) => State::Retry(retry),
                Err(UreqError::Transport(err))
                    if err.kind() == ureq::ErrorKind::ConnectionFailed =>
                {
                    State::Retry(retry)
                }
                Err(err) => State::Fail(
                    Report::new(ClientError)
                        .attach_printable(format!("Failed to say hello: {}", err)),
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

    #[tracing::instrument(skip(self, ctx))]
    fn request(
        &self,
        ctx: Ctx<Background>,
        capacity: u32,
    ) -> Result<Vec<PollResponse>, ClientError> {
        let endpoint = format!("{}/api/v0/jobs/request", self.url);
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
                .send_json(ureq::json!(PollRequest { capacity }))
            {
                Ok(res) => State::Done(res),
                Err(UreqError::Status(403, ..)) => State::Fail(
                    Report::new(FatalError)
                        .attach_printable("Unauthorized")
                        .change_context(ClientError),
                ),
                Err(UreqError::Status((500..), ..)) => State::Retry(retry),
                Err(UreqError::Transport(err))
                    if err.kind() == ureq::ErrorKind::ConnectionFailed =>
                {
                    State::Retry(retry)
                }
                Err(err) => State::Fail(
                    Report::new(ClientError)
                        .attach_printable(format!("Failed to poll jobs: {}", err)),
                ),
            }
        });

        match res {
            Ok(res) => {
                let res: Vec<PollResponse> = res
                    .into_json()
                    .change_context(ClientError)
                    .attach_printable("failed to read list of poll responses")?;
                Ok(res)
            }
            Err(recoil::recoil::Error::MaxRetriesReached) => Err(Report::new(RetryableError)
                .attach_printable("Max retries reached")
                .change_context(ClientError)),
            Err(recoil::recoil::Error::Custom(e)) => Err(e),
        }
    }

    #[tracing::instrument(skip(self, ctx))]
    fn complete(&self, ctx: Ctx<Background>, job_id: Uuid) -> Result<(), ClientError> {
        let endpoint = format!("{}/api/v0/jobs/complete", self.url);
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
                .send_json(ureq::json!(CompleteRequest { job_id }))
            {
                Ok(res) => State::Done(res),
                Err(UreqError::Status(403, ..)) => State::Fail(
                    Report::new(FatalError)
                        .attach_printable("Unauthorized")
                        .change_context(ClientError),
                ),
                Err(UreqError::Status((500..), ..)) => {
                    tracing::error!("Server error. Retrying...");
                    State::Retry(retry)
                }
                Err(UreqError::Transport(err))
                    if err.kind() == ureq::ErrorKind::ConnectionFailed =>
                {
                    tracing::error!("Connection failed: {}. Retrying...", err);
                    State::Retry(retry)
                }
                Err(err) => State::Fail(
                    Report::new(ClientError)
                        .attach_printable(format!("Failed to poll jobs: {}", err)),
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
}

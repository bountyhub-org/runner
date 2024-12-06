use crate::error::ClientError;
use crate::error::FatalError;
use crate::error::RetryableError;
use ctx::{Background, Ctx};
use error_stack::{Report, Result, ResultExt};
#[cfg(feature = "mockall")]
use mockall::mock;
use recoil::Interval;
use recoil::Recoil;
use recoil::State;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use ureq::Agent;
use ureq::Error as UreqError;

pub const DEFAULT_HUB_URL: &str = "https://bountyhub.org";

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationRequest {
    pub token: String,
    pub name: String,
    pub workdir: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationResponse {
    pub token: String,
    pub invoker_url: String,
    pub fluxy_url: String,
}

pub trait HubClient: Send + Sync + Clone {
    fn with_url(self, url: String) -> Self;

    fn register(
        &self,
        ctx: Ctx<Background>,
        request: &RegistrationRequest,
    ) -> Result<RegistrationResponse, ClientError>;
}

#[cfg(feature = "mockall")]
mock! {
    pub HubClient {}

    impl Clone for HubClient {
        fn clone(&self) -> Self;
    }

    impl HubClient for HubClient {
        fn with_url(self, url: String) -> Self;

        fn register(
            &self,
            ctx: Ctx<Background>,
            request: &RegistrationRequest,
        ) -> Result<RegistrationResponse, ClientError>;
    }
}

#[derive(Debug, Clone)]
pub struct HubHttpClient {
    url: String,
    agent: Agent,
    user_agent: String,
    recoil: Recoil,
}

impl HubHttpClient {
    #[tracing::instrument]
    pub fn new(user_agent: String, agent: Agent) -> Self {
        Self {
            agent,
            url: DEFAULT_HUB_URL.to_string(),
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
impl HubClient for HubHttpClient {
    #[tracing::instrument]
    fn with_url(self, url: String) -> Self {
        Self { url, ..self }
    }

    #[tracing::instrument(skip(ctx))]
    fn register(
        &self,
        ctx: Ctx<Background>,
        request: &RegistrationRequest,
    ) -> Result<RegistrationResponse, ClientError> {
        let endpoint = format!("{}/api/v0/runner-registrations/register", &self.url);
        let retry = || ctx.is_done();
        let mut recoil = self.recoil.clone();
        let res = recoil.run(|| {
            if ctx.is_done() {
                return State::Fail(Report::new(ClientError).attach_printable("cancelled"));
            }
            match self
                .agent
                .post(&endpoint)
                .set("Content-Type", "application/json")
                .set("User-Agent", &self.user_agent)
                .send_json(ureq::json!(request))
            {
                Ok(res) => State::Done(res),
                Err(UreqError::Status(409, _)) => State::Fail(
                    Report::new(FatalError)
                        .attach_printable("Conflict: runner already exists with that name")
                        .change_context(ClientError),
                ),
                Err(UreqError::Status(status, response)) => State::Fail(
                    Report::new(ClientError)
                        .attach_printable(format!("Response code {}: {:?}", status, response)),
                ),
                Err(UreqError::Transport(err))
                    if err.kind() == ureq::ErrorKind::ConnectionFailed =>
                {
                    tracing::debug!("Failed to connect to hub: {}", err);
                    State::Retry(retry)
                }

                Err(UreqError::Transport(err)) => State::Fail(
                    Report::new(ClientError)
                        .attach_printable(format!("Failed to connect to hub: {}", err)),
                ),
            }
        });

        match res {
            Ok(res) => {
                let reg: RegistrationResponse = res
                    .into_json()
                    .attach_printable("failed to parse json response")
                    .change_context(ClientError)?;

                tracing::debug!("Registered with hub: {:?}", reg);

                Ok(reg)
            }
            Err(recoil::recoil::Error::MaxRetriesReached) => Err(Report::new(RetryableError)
                .attach_printable("Max retries reached")
                .change_context(ClientError)),
            Err(recoil::recoil::Error::Custom(e)) => Err(e),
        }
    }
}

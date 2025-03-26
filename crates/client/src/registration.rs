use crate::error::{ClientError, OperationError};
use ctx::{Background, Ctx};
use miette::{miette, Result, WrapErr};
#[cfg(feature = "mockall")]
use mockall::mock;
use recoil::{Interval, Recoil, State};
use serde::{Deserialize, Serialize};
use std::time::Duration;

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

pub trait RegistrationClient: Send + Sync + Clone {
    fn register(
        &self,
        ctx: Ctx<Background>,
        request: &RegistrationRequest,
    ) -> Result<RegistrationResponse>;
}

#[cfg(feature = "mockall")]
mock! {
    pub RegistrationClient {}

    impl Clone for RegistrationClient {
        fn clone(&self) -> Self;
    }

    impl RegistrationClient for RegistrationClient {
        fn register(
            &self,
            ctx: Ctx<Background>,
            request: &RegistrationRequest,
        ) -> Result<RegistrationResponse>;
    }
}

#[derive(Debug, Clone)]
pub struct HttpRegistrationClient {
    client: ureq::Agent,
    url: String,
    user_agent: String,
    recoil: Recoil,
}

impl HttpRegistrationClient {
    pub fn new(client: ureq::Agent, url: &str, user_agent: &str) -> Self {
        Self {
            client,
            url: url.to_string(),
            user_agent: user_agent.to_string(),
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

impl RegistrationClient for HttpRegistrationClient {
    fn register(
        &self,
        ctx: Ctx<Background>,
        request: &RegistrationRequest,
    ) -> Result<RegistrationResponse> {
        let endpoint = format!("{}/api/v0/runner-registrations/register", &self.url);
        let retry = || ctx.is_done();
        let mut recoil = self.recoil.clone();
        let res = recoil.run(|| {
            if ctx.is_done() {
                return State::Fail(ClientError::CancellationError.into());
            }
            match self
                .client
                .post(&endpoint)
                .set("Content-Type", "application/json")
                .set("User-Agent", &self.user_agent)
                .send_json(ureq::json!(request))
                .map_err(ClientError::from)
            {
                Ok(res) => State::Done(res),
                Err(e @ ClientError::ConflictError) => {
                    State::Fail(miette!("Runner with the same name already exists").wrap_err(e))
                }
                Err(ClientError::ServerError | ClientError::ConnectionError(..)) => {
                    State::Retry(retry)
                }
                Err(e) => State::Fail(miette!("Failed to register the runner").wrap_err(e)),
            }
        });

        match res {
            Ok(res) => {
                let reg: RegistrationResponse = res
                    .into_json()
                    .map_err(OperationError::from)
                    .wrap_err("Failed to deserialize job resolved response")?;

                tracing::debug!("Registered with hub: {:?}", reg);

                Ok(reg)
            }
            Err(recoil::recoil::Error::MaxRetriesReachedError) => {
                Err(OperationError::MaxRetriesError.into())
            }
            Err(recoil::recoil::Error::UserError(e)) => Err(e),
        }
    }
}

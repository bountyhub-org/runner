use ctx::{Background, Ctx};
use miette::Result;
#[cfg(feature = "mockall")]
use mockall::mock;
use recoil::{Recoil, State};
use serde::{Deserialize, Serialize};

use crate::error::ClientError;

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationRequest {
    pub token: String,
    pub name: String,
    pub workdir: String,
    pub replace: bool,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationResponse {
    pub token: String,
    pub invoker_url: String,
    pub fluxy_url: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseResponse {
    pub version: String,
}

pub trait BountyHubClient: Send + Sync + Clone {
    fn register(
        &self,
        ctx: Ctx<Background>,
        request: &RegistrationRequest,
    ) -> Result<RegistrationResponse>;

    fn get_latest_runner_release(&self, ctx: Ctx<Background>) -> Result<ReleaseResponse>;
}

#[cfg(feature = "mockall")]
mock! {
    pub BountyHubClient {}

    impl Clone for BountyHubClient {
        fn clone(&self) -> Self;
    }

    impl BountyHubClient for BountyHubClient {
        fn register(
            &self,
            ctx: Ctx<Background>,
            request: &RegistrationRequest,
        ) -> Result<RegistrationResponse>;

        fn get_latest_runner_release(&self, ctx: Ctx<Background>) -> Result<ReleaseResponse>;
    }
}

#[derive(Debug, Clone)]
pub struct HttpBountyHubClient {
    pub url: String,
    pub client: ureq::Agent,
    pub recoil: Recoil,
}

impl BountyHubClient for HttpBountyHubClient {
    fn register(
        &self,
        ctx: Ctx<Background>,
        req: &RegistrationRequest,
    ) -> Result<RegistrationResponse> {
        let endpoint = format!("{}/api/v0/runner-registrations/register", self.url);

        let retry = || ctx.is_done();
        let mut result = self
            .recoil
            .run(|| {
                if ctx.is_done() {
                    return State::Fail(ClientError::CancellationError);
                }
                match self
                    .client
                    .post(&endpoint)
                    .header("Content-Type", "application/json")
                    .send_json(req)
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
            .read_json::<RegistrationResponse>()
            .map_err(ClientError::from)?)
    }

    fn get_latest_runner_release(&self, ctx: Ctx<Background>) -> Result<ReleaseResponse> {
        let endpoint = format!("{}/api/v0/runners/releases/latest", self.url);
        let retry = || ctx.is_done();
        let mut result = self
            .recoil
            .run(|| {
                if ctx.is_done() {
                    return State::Fail(ClientError::CancellationError);
                }
                match self
                    .client
                    .get(&endpoint)
                    .header("Content-Type", "application/json")
                    .call()
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
            .read_json::<ReleaseResponse>()
            .map_err(ClientError::from)?)
    }
}

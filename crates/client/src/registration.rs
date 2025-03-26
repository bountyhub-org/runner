use ctx::{Background, Ctx};
use miette::Result;
#[cfg(feature = "mockall")]
use mockall::mock;
use serde::{Deserialize, Serialize};

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

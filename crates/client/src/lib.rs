pub mod error;
pub mod hub;
pub mod runner;
pub mod worker;

#[cfg(feature = "mockall")]
use hub::MockHubClient;
use hub::{HubClient, HubHttpClient};
#[cfg(feature = "mockall")]
use mockall::mock;
#[cfg(feature = "mockall")]
use runner::MockRunnerClient;
use runner::{RunnerClient, RunnerHttpClient};
use std::time::Duration;
#[cfg(feature = "mockall")]
use worker::MockWorkerClient;
use worker::{WorkerClient, WorkerHttpClient};

pub trait ClientBuilder: Send + Sync + Clone {
    type Hub: HubClient;
    type Invoker: RunnerClient;
    type Worker: WorkerClient;

    fn hub(&self) -> Self::Hub;

    fn invoker(&self) -> Self::Invoker;

    fn worker(&self) -> Self::Worker;
}

#[cfg(feature = "mockall")]
mock! {
    pub ClientBuilder {}

    impl Clone for ClientBuilder {
        fn clone(&self) -> Self;
    }

    impl ClientBuilder for ClientBuilder {
        type Hub = MockHubClient;
        type Invoker = MockRunnerClient;
        type Worker = MockWorkerClient;

        fn hub(&self) -> <MockClientBuilder as ClientBuilder>::Hub;
        fn invoker(&self) -> <MockClientBuilder as ClientBuilder>::Invoker;
        fn worker(&self) -> <MockClientBuilder as ClientBuilder>::Worker;
    }
}

#[derive(Debug, Clone)]
pub struct HttpClientBuilder {
    /// The client used for long-polling requests.
    /// This client has a longer read timeout than the default client.
    long_poll_client: ureq::Agent,

    /// The default client used for all other requests.
    default_client: ureq::Agent,

    /// The user agent to use for all requests.
    /// User agent is used to propagate telemetry information
    /// and should be used to better understand issues related
    /// to specific versions of the runner.
    user_agent: String,
}

#[derive(Debug, Clone)]
pub struct HttpClientBuilderConfig {
    pub user_agent: String,
}

impl HttpClientBuilder {
    #[tracing::instrument]
    pub fn new(config: HttpClientBuilderConfig) -> Self {
        Self {
            long_poll_client: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(5 * 60))
                .timeout_write(Duration::from_secs(2 * 60))
                .build(),
            default_client: ureq::AgentBuilder::new()
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(30))
                .timeout_write(Duration::from_secs(30))
                .build(),
            user_agent: config.user_agent,
        }
    }
}

impl ClientBuilder for HttpClientBuilder {
    type Hub = HubHttpClient;
    type Invoker = RunnerHttpClient;
    type Worker = WorkerHttpClient;

    #[tracing::instrument]
    fn hub(&self) -> Self::Hub {
        HubHttpClient::new(self.user_agent.clone(), self.default_client.clone())
    }

    #[tracing::instrument]
    fn invoker(&self) -> Self::Invoker {
        RunnerHttpClient::new(self.user_agent.clone(), self.long_poll_client.clone())
    }

    #[tracing::instrument]
    fn worker(&self) -> Self::Worker {
        WorkerHttpClient::new(self.user_agent.clone(), self.default_client.clone())
    }
}

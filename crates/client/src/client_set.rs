use crate::bountyhub::HttpBountyHubClient;
use crate::runner::HttpRunnerClient;
use crate::worker::HttpWorkerClient;
use config::ConfigManager;
use recoil::Recoil;
use std::{sync::Arc, time::Duration};
use ureq::{
    middleware::Middleware,
    tls::{RootCerts, TlsConfig},
};

#[derive(Debug, Clone)]
pub struct ClientSet {
    inner: Arc<InnerClientSet>,
}

#[derive(Debug, Clone)]
pub struct ClientSetConfig {
    pub user_agent: String,
    pub config_manager: ConfigManager,
    pub recoil: Recoil,
}

#[derive(Debug, Clone)]
struct InnerClientSet {
    runner_client: HttpRunnerClient,
    worker_client: HttpWorkerClient,
    bountyhub_client: HttpBountyHubClient,
}

impl InnerClientSet {
    fn new(cfg: ClientSetConfig) -> Self {
        let tls = TlsConfig::builder()
            .root_certs(RootCerts::PlatformVerifier)
            .build();
        Self {
            runner_client: HttpRunnerClient {
                recoil: cfg.recoil,
                config_manager: cfg.config_manager.clone(),
                default_client: ureq::Agent::new_with_config(
                    ureq::Agent::config_builder()
                        .timeout_send_request(Some(Duration::from_secs(30)))
                        .timeout_recv_response(Some(Duration::from_secs(30)))
                        .timeout_connect(Some(Duration::from_secs(10)))
                        .user_agent(&cfg.user_agent)
                        .tls_config(tls.clone())
                        .middleware(TokenRefreshMiddleware {
                            config_manager: cfg.config_manager.clone(),
                        })
                        .build(),
                ),
                long_poll_client: ureq::Agent::new_with_config(
                    ureq::Agent::config_builder()
                        .timeout_send_request(Some(Duration::from_secs(2 * 60)))
                        .timeout_recv_response(Some(Duration::from_secs(2 * 60)))
                        .timeout_connect(Some(Duration::from_secs(10)))
                        .user_agent(&cfg.user_agent)
                        .tls_config(tls.clone())
                        .middleware(TokenRefreshMiddleware {
                            config_manager: cfg.config_manager.clone(),
                        })
                        .build(),
                ),
            },
            worker_client: HttpWorkerClient {
                recoil: cfg.recoil,
                config_manager: cfg.config_manager.clone(),
                default_client: ureq::Agent::new_with_config(
                    ureq::Agent::config_builder()
                        .timeout_send_request(Some(Duration::from_secs(30)))
                        .timeout_recv_response(Some(Duration::from_secs(30)))
                        .timeout_connect(Some(Duration::from_secs(10)))
                        .user_agent(&cfg.user_agent)
                        .tls_config(tls.clone())
                        .build(),
                ),
                artifact_client: ureq::Agent::new_with_config(
                    ureq::Agent::config_builder()
                        .timeout_send_request(Some(Duration::from_secs(2 * 60)))
                        .timeout_recv_response(Some(Duration::from_secs(2 * 60)))
                        .timeout_connect(Some(Duration::from_secs(10)))
                        .user_agent(&cfg.user_agent)
                        .tls_config(tls.clone())
                        .build(),
                ),
                token: String::default(),
            },
            bountyhub_client: HttpBountyHubClient {
                recoil: cfg.recoil,
                client: ureq::Agent::new_with_config(
                    ureq::Agent::config_builder()
                        .timeout_send_request(Some(Duration::from_secs(30)))
                        .timeout_recv_response(Some(Duration::from_secs(30)))
                        .timeout_connect(Some(Duration::from_secs(10)))
                        .user_agent(&cfg.user_agent)
                        .tls_config(tls.clone())
                        .build(),
                ),
                url: String::default(),
            },
        }
    }
}

impl ClientSet {
    pub fn new(cfg: ClientSetConfig) -> Self {
        Self {
            inner: Arc::new(InnerClientSet::new(cfg)),
        }
    }

    #[inline]
    pub fn runner_client(&self) -> HttpRunnerClient {
        self.inner.runner_client.clone()
    }

    #[inline]
    pub fn worker_client(&self, token: &str) -> HttpWorkerClient {
        let mut client = self.inner.worker_client.clone();
        client.token = token.to_string();
        client
    }

    #[inline]
    pub fn bountyhub_client(&self, url: &str) -> HttpBountyHubClient {
        let mut client = self.inner.bountyhub_client.clone();
        client.url = url.to_string();
        client
    }
}

impl Default for ClientSet {
    fn default() -> Self {
        let cfg = ClientSetConfig {
            user_agent: format!("runner/{} (cli)", env!("CARGO_PKG_VERSION")),
            recoil: Recoil::default(),
            config_manager: ConfigManager::default(),
        };

        Self::new(cfg)
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

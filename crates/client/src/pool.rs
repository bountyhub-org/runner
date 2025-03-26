use std::{sync::Arc, time::Duration};

#[derive(Debug, Clone)]
pub struct Client {
    inner: Arc<InnerClientPool>,
}

#[derive(Debug, Clone)]
struct InnerClientPool {
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
}

impl InnerClientPool {
    fn new(cfg: PoolConfig) -> Self {
        Self {
            long_poll_client: cfg.long_poll_client.agent(),
            assets_client: cfg.assets_client.agent(),
            default_client: cfg.default_client.agent(),
            stream_client: cfg.stream_client.agent(),
        }
    }
}

impl Client {
    pub fn new(cfg: PoolConfig) -> Self {
        Self {
            inner: Arc::new(InnerClientPool::new(cfg)),
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

impl Default for Client {
    fn default() -> Self {
        let user_agent = format!("runner/{} (cli)", env!("CARGO_PKG_VERSION"));
        Self::new(PoolConfig {
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
pub struct PoolConfig {
    pub default_client: ClientConfig,
    pub long_poll_client: ClientConfig,
    pub assets_client: ClientConfig,
    pub stream_client: ClientConfig,
}


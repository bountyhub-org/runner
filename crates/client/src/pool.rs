use std::{sync::Arc, time::Duration};

#[derive(Debug, Clone)]
pub struct ClientPool {
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
            long_poll_client: ureq::AgentBuilder::new()
                .timeout_connect(cfg.long_poll_client.timeout_connect)
                .timeout_read(cfg.long_poll_client.timeout_read)
                .timeout_write(cfg.long_poll_client.timeout_write)
                .build(),

            assets_client: ureq::AgentBuilder::new()
                .timeout_connect(cfg.assets_client.timeout_connect)
                .timeout_read(cfg.assets_client.timeout_read)
                .timeout_write(cfg.assets_client.timeout_write)
                .build(),

            default_client: ureq::AgentBuilder::new()
                .timeout_connect(cfg.default_client.timeout_connect)
                .timeout_read(cfg.default_client.timeout_read)
                .timeout_write(cfg.default_client.timeout_write)
                .build(),

            stream_client: ureq::AgentBuilder::new()
                .timeout_connect(cfg.stream_client.timeout_connect)
                .timeout_read(cfg.stream_client.timeout_read)
                .timeout_write(cfg.stream_client.timeout_write)
                .build(),
        }
    }
}

impl ClientPool {
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

impl Default for ClientPool {
    fn default() -> Self {
        Self::new(PoolConfig {
            default_client: ClientConfig {
                timeout_connect: Duration::from_secs(10),
                timeout_read: Duration::from_secs(30),
                timeout_write: Duration::from_secs(30),
            },
            long_poll_client: ClientConfig {
                timeout_connect: Duration::from_secs(10),
                timeout_read: Duration::from_secs(2 * 60),
                timeout_write: Duration::from_secs(10),
            },
            assets_client: ClientConfig {
                timeout_connect: Duration::from_secs(10),
                timeout_read: Duration::from_secs(2 * 60),
                timeout_write: Duration::from_secs(2 * 60),
            },
            stream_client: ClientConfig {
                timeout_connect: Duration::from_secs(10),
                timeout_read: Duration::from_secs(45 * 60),
                timeout_write: Duration::from_secs(45 * 60),
            },
        })
    }
}

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub timeout_connect: Duration,
    pub timeout_read: Duration,
    pub timeout_write: Duration,
}

#[derive(Debug, Clone)]
pub struct PoolConfig {
    pub default_client: ClientConfig,
    pub long_poll_client: ClientConfig,
    pub assets_client: ClientConfig,
    pub stream_client: ClientConfig,
}

impl ClientPool {
    pub fn new(cfg: PoolConfig) -> Self {
        Self {
            inner: Arc::new(InnerClientPool::new(cfg)),
        }
    }
}

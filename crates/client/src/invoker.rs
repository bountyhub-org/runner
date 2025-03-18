use futures_util::{SinkExt, StreamExt};
use miette::{IntoDiagnostic, Result, WrapErr};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use reqwest_tracing::TracingMiddleware;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

pub struct Client {
    cfg: Config,
    client: ClientWithMiddleware,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub user_agent: String,
    pub url: String,
}

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

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
enum RunnerRequestEvent {
    AcquireJobs { capacity: u32 },
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub enum RunnerResponseEvent {
    UpgradeRequired { version: String },
    AcquiredJobs { jobs: Vec<Uuid> },
}

impl Client {
    pub fn new(cfg: Config) -> Self {
        let retry_policy = ExponentialBackoff::builder().build_with_max_retries(5);
        let client = ClientBuilder::new(reqwest::Client::new())
            .with(TracingMiddleware::default())
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .build();

        Self { cfg, client }
    }

    pub async fn register(&self, request: &RegistrationRequest) -> Result<RegistrationResponse> {
        let endpoint = format!("{}/api/v0/runner-registrations/register", &self.cfg.url);

        self.client
            .post(&endpoint)
            .header("Content-Type", "application/json")
            .header("User-Agent", &self.cfg.user_agent)
            .json(request)
            .send()
            .await
            .into_diagnostic()?
            .json()
            .await
            .into_diagnostic()
    }

    pub async fn runner_connect(
        &self,
    ) -> Result<(Sender<RunnerRequestEvent>, Receiver<RunnerResponseEvent>)> {
        let (stream, _) = tokio_tungstenite::connect_async(&format!(
            "wss://{}/api/v0/runners/connect",
            &self.cfg.url
        ))
        .await
        .into_diagnostic()?;

        let (recv_tx, recv_rx) = mpsc::channel(1);
        let (send_tx, mut send_rx) = mpsc::channel(1);

        let (mut sender, mut receiver) = stream.split();

        tokio::spawn(async move {
            while let Some(Ok(msg)) = receiver.next().await {
                match msg {
                    Message::Text(utf8_bytes) => {
                        match serde_json::from_str(utf8_bytes.as_str()) {
                            Ok(event) => {
                                if let Err(e) = recv_tx.send(event).await {
                                    tracing::error!("Failed to send the runner response message, finishing: {e:?}");
                                    break;
                                }
                            }
                            Err(e) => {
                                tracing::info!("Failed to deserialize the message: {e:?}");
                                continue;
                            }
                        }
                    }
                    Message::Close(_) => {
                        tracing::info!("Receiver stream closed");
                        break;
                    }
                    _ => continue,
                };
            }
        });

        tokio::spawn(async move {
            while let Some(event) = send_rx.recv().await {
                match serde_json::to_string(&event) {
                    Ok(event) => {
                        if let Err(e) = sender.send(Message::Text(event.into())).await {
                            tracing::error!("Failed to send the message: {e:?}");
                            return;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to serialize event: {e:?}");
                        continue;
                    }
                };
            }
        });

        Ok((send_tx, recv_rx))
    }
}

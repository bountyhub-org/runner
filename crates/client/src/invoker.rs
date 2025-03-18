use futures::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use jobengine::{JobMeta, ProjectMeta, WorkflowMeta, WorkflowRevisionMeta};
use miette::{IntoDiagnostic, Result, WrapErr};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{policies::ExponentialBackoff, RetryTransientMiddleware};
use reqwest_tracing::TracingMiddleware;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::collections::BTreeMap;
use time::OffsetDateTime;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tokio_tungstenite::{tungstenite::Message, MaybeTlsStream, WebSocketStream};
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
#[serde(tag = "kind", content = "data")]
pub enum RunnerRequestEvent {
    AcquireJobs { capacity: u32 },
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "kind", content = "data")]
pub enum RunnerResponseEvent {
    UpgradeRequired { version: String },
    AcquiredJobs { jobs: Vec<Uuid> },
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "kind", content = "data")]
pub enum WorkerRequestEvent {
    ResolveJob { id: Uuid, token: String },
    SendLogLine { line: LogLine },
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
#[serde(tag = "kind", content = "data")]
pub enum WorkerResponseEvent {
    JobResolved(JobResolvedResponse),
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobResolvedResponse {
    pub steps: Vec<Step>,
    pub cfg: jobengine::Config,
}

#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Step {
    Setup,
    Teardown,
    Upload {
        uploads: Vec<String>,
    },
    Command {
        cond: String,
        run: String,
        shell: String,
        allow_failed: bool,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobExecutionContext {
    pub scans: BTreeMap<String, Vec<JobMeta>>,
    pub vars: BTreeMap<String, String>,
    pub project: ProjectMeta,
    pub workflow: WorkflowMeta,
    pub revision: WorkflowRevisionMeta,
}

#[derive(Serialize, Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct JobContext {
    pub id: Uuid,
    pub state: String,
    pub nonce: Option<String>,
}

#[derive(Serialize, Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProjectContext {
    pub id: Uuid,
}

#[derive(Serialize, Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowContext {
    pub id: Uuid,
}

#[derive(Serialize, Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RevisionContext {
    pub id: Uuid,
}

#[derive(Debug, Clone, Copy, Serialize_repr, Deserialize_repr)]
#[repr(u8)]
pub enum LogDestination {
    Stdout = 1,
    Stderr = 2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub dst: LogDestination,
    pub step_index: u32,
    #[serde(with = "time::serde::rfc3339")]
    pub timestamp: OffsetDateTime,
    pub line: String,
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
            .into_diagnostic()
            .wrap_err("Failed to send the message")?
            .json()
            .await
            .into_diagnostic()
            .wrap_err("Failed to deserialize response")
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
        let (send_tx, send_rx) = mpsc::channel(1);

        let (sender, receiver) = stream.split();

        tokio::spawn(async move { Self::ws_recv(receiver, recv_tx, "Runner").await });

        tokio::spawn(async move {
            Self::ws_send(sender, send_rx, "Runner").await;
        });

        Ok((send_tx, recv_rx))
    }

    pub async fn worker_connect(
        &self,
    ) -> Result<(Sender<WorkerRequestEvent>, Receiver<WorkerResponseEvent>)> {
        let (stream, _) = tokio_tungstenite::connect_async(&format!(
            "wss://{}/api/v0/workers/connect",
            &self.cfg.url
        ))
        .await
        .into_diagnostic()?;

        let (recv_tx, recv_rx) = mpsc::channel(1);
        let (send_tx, send_rx) = mpsc::channel(1);

        let (sender, receiver) = stream.split();

        tokio::spawn(async move { Self::ws_recv(receiver, recv_tx, "Worker").await });

        tokio::spawn(async move {
            Self::ws_send(sender, send_rx, "Worker").await;
        });

        Ok((send_tx, recv_rx))
    }

    #[tracing::instrument(skip(receiver, tx))]
    async fn ws_recv<T>(
        mut receiver: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        tx: Sender<T>,
        component: &'static str,
    ) where
        T: DeserializeOwned,
    {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Text(utf8_bytes) => match serde_json::from_str(utf8_bytes.as_str()) {
                    Ok(event) => {
                        if let Err(e) = tx.send(event).await {
                            tracing::error!(
                                "Failed to send the runner response message, finishing: {e:?}"
                            );
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::info!("Failed to deserialize the message: {e:?}");
                        continue;
                    }
                },
                Message::Close(_) => {
                    tracing::info!("Receiver stream closed");
                    break;
                }
                _ => continue,
            };
        }
    }

    #[tracing::instrument(skip(sender, rx))]
    async fn ws_send<T>(
        mut sender: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
        mut rx: Receiver<T>,
        component: &'static str,
    ) where
        T: Serialize,
    {
        while let Some(event) = rx.recv().await {
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
    }
}

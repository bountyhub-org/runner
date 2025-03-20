use client::invoker::Client as InvokerClient;
use client::runner::{JobAcquiredResponse, RunnerClient};
use config::ConfigManager;
use ctx::{Background, Ctx};
use miette::{miette, IntoDiagnostic, Result};
#[cfg(test)]
use mockall::automock;
use std::collections::BTreeMap;
use std::fs;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use worker::Worker;

#[derive(Debug, Clone)]
pub struct Runner<WB, W>
where
    WB: WorkerBuilder<Worker = W>,
    W: Worker,
{
    config: ConfigManager,
    worker_builder: WB,
}

#[cfg_attr(test, automock)]
impl<WB, W> Runner<WB, W>
where
    WB: WorkerBuilder<Worker = W> + 'static,
    W: Worker + 'static,
{
    pub fn new(config: ConfigManager, worker_builder: WB) -> Self {
        Self {
            config,
            worker_builder,
        }
    }

    #[tracing::instrument(skip(self, client))]
    pub async fn run(&self, client: InvokerClient) -> Result<()> {
        tracing::info!("Initializing working directory");
        fs::create_dir_all(&self.config.read().unwrap().workdir)
            .await
            .into_diagnostic()?;

        tracing::info!("Connecting to the invoker service");
        let (tx, rx) = client.runner_connect().await?;

        todo!()
    }
}

pub trait WorkerBuilder {
    type Worker: worker::Worker;
    fn build(&self, job: JobAcquiredResponse) -> Self::Worker;
}

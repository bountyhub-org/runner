use client::invoker::Client as InvokerClient;
use client::runner::{JobAcquiredResponse, RunnerClient};
use config::Config;
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
use uuid::Uuid;
use worker::Worker;

#[derive(Debug, Clone)]
pub struct Runner<WB, W>
where
    WB: WorkerBuilder<Worker = W>,
    W: Worker,
{
    config: Arc<RwLock<Config>>,
    worker_builder: WB,
}

#[cfg_attr(test, automock)]
impl<WB, W> Runner<WB, W>
where
    WB: WorkerBuilder<Worker = W> + 'static,
    W: Worker + 'static,
{
    pub fn new(config: Arc<RwLock<Config>>, worker_builder: WB) -> Self {
        Self {
            config,
            worker_builder,
        }
    }

    #[tracing::instrument(skip(self, ctx, client))]
    pub async fn run(&self, ctx: Ctx<Background>, client: InvokerClient) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use client::error::ClientError;
    use client::runner::MockRunnerClient;
    use miette::bail;

    #[test]
    fn test_poll_loop() {
        let mut client = MockRunnerClient::new();
        client
            .expect_request()
            .times(1)
            .returning(|_, _| bail!("error"));

        let (poll_tx, _poll_rx) =
            mpsc::sync_channel::<Result<Vec<JobAcquiredResponse>, ClientError>>(1);

        let (_worker_finished_tx, worker_finished_rx) = mpsc::sync_channel::<Uuid>(1);

        let handle = thread::spawn(move || {
            poll_loop(ctx::background(), client, 1, poll_tx, worker_finished_rx);
        });

        thread::sleep(Duration::from_secs(1));
        assert!(
            handle.is_finished(),
            "expected poll to exit after fatal error"
        );
        handle.join().expect("handle should be joined properly");
    }
}

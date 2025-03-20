use client::invoker::Client as InvokerClient;
use client::invoker::JobAcquiredResponse;
use client::invoker::RunnerRequestEvent;
use client::invoker::RunnerResponseEvent;
use config::ConfigManager;
use miette::bail;
use miette::{IntoDiagnostic, Result, WrapErr};
#[cfg(test)]
use mockall::automock;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::fs;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
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
    WB: WorkerBuilder<Worker = W> + 'static + Send + Sync,
    W: Worker + 'static,
{
    pub fn new(config: ConfigManager, worker_builder: WB) -> Self {
        Self {
            config,
            worker_builder,
        }
    }

    #[tracing::instrument(skip(self, client))]
    pub async fn run(self, ct: CancellationToken, client: InvokerClient) -> Result<()> {
        tracing::info!("Initializing working directory");
        fs::create_dir_all(&self.config.get().await?.workdir)
            .await
            .into_diagnostic()?;

        tracing::info!("Connecting to the invoker service");
        let (tx, mut rx) = client.runner_connect().await?;

        let capacity = self.config.get().await?.capacity;
        let worker_handles = Arc::new(Mutex::new(Some(BTreeMap::new())));
        let (worker_tx, mut worker_rx) = mpsc::channel(1);

        let listener_worker_handles = Arc::clone(&worker_handles);
        let listener_handle = tokio::spawn(async move {
            let worker_handles = listener_worker_handles;
            let capacity = capacity as usize;
            loop {
                let capacity = capacity
                    - worker_handles
                        .lock()
                        .await
                        .as_ref()
                        .expect("When lock is held, value must be some")
                        .len();

                tx.send(RunnerRequestEvent::AcquireJobs {
                    capacity: capacity as u32,
                })
                .await
                .into_diagnostic()
                .wrap_err("Failed to send acquire jobs")?;

                match rx.recv().await {
                    Some(RunnerResponseEvent::AcquiredJobs { jobs }) => {
                        worker_tx
                            .send(jobs)
                            .await
                            .into_diagnostic()
                            .wrap_err("Failed to push scheduled jobs")?;
                    }
                    Some(RunnerResponseEvent::UpgradeRequired { version: _version }) => {
                        break Ok(())
                    }
                    None => {
                        tracing::error!("Channel failed during running");
                        bail!("Failed to read from the channel");
                    }
                };

                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });

        let worker_builder = self.worker_builder;
        let worker_ct = ct.clone();
        let worker_spawn_handles = Arc::clone(&worker_handles);
        let worker_spawn_handle: JoinHandle<Result<()>> = tokio::spawn(async move {
            let worker_ct = worker_ct;
            let worker_builder = worker_builder;
            let worker_handles = worker_spawn_handles;

            while let Some(jobs) = worker_rx.recv().await {
                let mut worker_handles = worker_handles.lock().await;
                let mut handles = worker_handles.take().unwrap();
                for job in jobs {
                    let id = job.id;
                    let ct = worker_ct.clone();
                    let worker = worker_builder.build(job);
                    let handle = tokio::spawn(async move { worker.run(ct).await });
                    handles.insert(id, handle);
                }
                *worker_handles = Some(handles);
            }

            Ok(())
        });

        let worker_join_handles = Arc::clone(&worker_handles);
        let worker_ct = ct.clone();
        let worker_wait_handle: JoinHandle<Result<()>> = tokio::spawn(async move {
            loop {
                if worker_ct.is_cancelled() {
                    return Ok(());
                }

                {
                    let mut worker_handles = worker_join_handles.lock().await;
                    let mut handles = worker_handles
                        .take()
                        .expect("If lock is held, value must be Some");

                    for (id, handle) in &mut handles {
                        if !handle.is_finished() {
                            continue;
                        }

                        if let Err(e) = handle.await {
                            tracing::error!(
                                "Worker handle running job '{id}' returned an error: {e:?}"
                            );
                        }
                    }

                    handles.retain(|_, handle| !handle.is_finished());

                    *worker_handles = Some(handles);
                }

                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });

        let (listener, worker_spawn, worker_wait) =
            tokio::join!(listener_handle, worker_spawn_handle, worker_wait_handle);

        listener
            .into_diagnostic()?
            .wrap_err("Listener returned an error")?;

        worker_spawn
            .into_diagnostic()?
            .wrap_err("Worker spawn returned an error")?;

        worker_wait
            .into_diagnostic()?
            .wrap_err("Worker wait returned an error")?;

        Ok(())
    }
}

pub trait WorkerBuilder {
    type Worker: worker::Worker;
    fn build(&self, job: JobAcquiredResponse) -> Self::Worker;
}

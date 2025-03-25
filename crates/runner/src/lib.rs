use client::error::ClientError;
use client::runner::{JobAcquiredResponse, RunnerClient};
use config::ConfigManager;
use ctx::{Background, Ctx};
use miette::{miette, IntoDiagnostic, Result};
#[cfg(test)]
use mockall::automock;
use std::collections::BTreeMap;
use std::fs;
use std::sync::mpsc::{self, Receiver, SyncSender};
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
    config_manager: ConfigManager,
    worker_builder: WB,
}

#[derive(Debug, Clone)]
struct RunnerGreeting<RC>
where
    RC: RunnerClient,
{
    runner_client: RC,
}

impl<RC> RunnerGreeting<RC>
where
    RC: RunnerClient,
{
    fn new(runner_client: RC) -> Self {
        Self { runner_client }
    }

    #[tracing::instrument(skip(self))]
    fn hello(&mut self, ctx: Ctx<Background>) -> Result<()> {
        self.runner_client.hello(ctx.to_background())
    }
}

impl<RC> Drop for RunnerGreeting<RC>
where
    RC: RunnerClient,
{
    #[tracing::instrument(skip(self))]
    fn drop(&mut self) {
        if let Err(err) = self.runner_client.goodbye(ctx::background()) {
            tracing::error!(
                "Failed to send goodbye message to invoker service: {:?}",
                err
            );
        }
    }
}

#[cfg_attr(test, automock)]
impl<WB, W> Runner<WB, W>
where
    WB: WorkerBuilder<Worker = W> + 'static,
    W: Worker + 'static,
{
    pub fn new(config_manager: ConfigManager, worker_builder: WB) -> Self {
        Self {
            config_manager,
            worker_builder,
        }
    }

    #[tracing::instrument(skip(self, ctx, client))]
    pub fn run<C>(&self, ctx: Ctx<Background>, client: C) -> Result<()>
    where
        C: RunnerClient + 'static,
    {
        tracing::info!("Initializing working directory");
        fs::create_dir_all(&self.config_manager.get()?.workdir).into_diagnostic()?;

        tracing::info!("Contacting invoker service");
        let mut greeting = RunnerGreeting::new(client.clone());
        greeting.hello(ctx.clone())?;

        tracing::info!("Listening for jobs");

        let root_ctx = ctx.with_cancel();
        let wait_ctx = root_ctx.to_background();
        let poll_ctx = root_ctx.to_background();
        let result_ctx = root_ctx.to_background();

        let channel_cap = self.config_manager.get()?.capacity as usize;
        let (worker_started_tx, worker_started_rx) =
            mpsc::sync_channel::<Vec<(Uuid, JoinHandle<()>)>>(channel_cap);
        let (worker_finished_tx, worker_finished_rx) = mpsc::sync_channel::<Uuid>(channel_cap);

        let wait_handle = thread::spawn(move || {
            let mut handles = BTreeMap::new();
            let ctx = wait_ctx;
            loop {
                if ctx.is_done() && handles.is_empty() {
                    break;
                }

                let hs = match worker_started_rx.try_recv() {
                    Ok(hs) => Some(hs),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        tracing::error!("Worker channel is disconnected");
                        break;
                    }
                };

                if let Some(hs) = hs {
                    tracing::info!("Received worker handles: {}", hs.len());
                    for h in hs {
                        handles.insert(h.0, h.1);
                    }
                }

                let mut to_remove = Vec::new();
                for (id, handle) in handles.iter() {
                    if handle.is_finished() {
                        to_remove.push(*id);
                    }
                }

                for id in to_remove {
                    tracing::info!("Joining worker thread: {}", id);
                    worker_finished_tx.send(id).unwrap();
                    let handle = handles.remove(&id).unwrap();
                    if let Err(err) = handle.join() {
                        tracing::error!("Failed to join worker thread: {:?}", err);
                        continue;
                    }
                }

                thread::sleep(Duration::from_millis(100));
            }
        });

        let (poll_tx, poll_rx) =
            mpsc::sync_channel::<Result<Vec<JobAcquiredResponse>, ClientError>>(1);
        let poll_client = client.clone();
        let capacity = self.config_manager.get()?.capacity;
        let _poll_handle = thread::spawn(move || {
            poll_loop(poll_ctx, poll_client, capacity, poll_tx, worker_finished_rx);
            root_ctx.cancel();
        });

        let result = loop {
            if result_ctx.is_done() {
                tracing::info!("Runner context is cancelled");
                break Ok(());
            }

            let jobs = match poll_rx.try_recv() {
                Ok(Ok(jobs)) => jobs,
                Ok(Err(err)) => {
                    tracing::error!("Failed to poll jobs: {:?}", err);
                    break Err(err.into());
                }
                Err(mpsc::TryRecvError::Empty) => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    tracing::error!("Poll channel is disconnected");
                    break Err(miette!("channel disconnected"));
                }
            };

            tracing::debug!("Poll jobs: {:?}", jobs);

            let id_handles = jobs
                .into_iter()
                .map(|job| {
                    let id = job.id;
                    let ctx = ctx.to_background();
                    let worker = self.worker_builder.build(job).unwrap();
                    let runner_client = client.clone();
                    let handle = thread::spawn(move || {
                        if let Err(err) = worker.run(ctx.clone()) {
                            tracing::error!("Worker failed: {:?}", err);
                            return;
                        }
                        if let Err(err) = runner_client.complete(ctx, id) {
                            tracing::error!("Failed to complete job: {:?}", err);
                        }
                    });
                    (id, handle)
                })
                .collect::<Vec<_>>();

            if let Err(err) = worker_started_tx.send(id_handles) {
                tracing::error!("Failed to send worker handles: {:?}", err);
            }
        };

        tracing::info!("Waiting for worker threads to finish");
        wait_handle.join().expect("Failed to join wait thread");
        result
    }
}

fn poll_loop<RC>(
    ctx: Ctx<Background>,
    client: RC,
    mut capacity: u32,
    poll_tx: SyncSender<Result<Vec<JobAcquiredResponse>, ClientError>>,
    worker_finished_rx: Receiver<Uuid>,
) where
    RC: RunnerClient,
{
    loop {
        if ctx.is_done() {
            tracing::info!("Polling context is cancelled");
            return;
        }

        while let Ok(jobs) = worker_finished_rx.try_recv() {
            tracing::info!("Worker finished: {}", jobs);
            capacity += 1;
        }

        if capacity == 0 {
            thread::sleep(Duration::from_millis(100));
            continue;
        }

        let jobs = client.request(ctx.to_background(), capacity);
        match jobs {
            Ok(jobs) => {
                if jobs.is_empty() {
                    continue;
                }
                capacity -= jobs.len() as u32;
                poll_tx.send(Ok(jobs)).unwrap();
            }
            Err(e) => {
                match e.downcast::<ClientError>() {
                    Ok(e) => match e {
                        ClientError::ServerError | ClientError::ConnectionError(..) => {
                            tracing::error!("Failed to poll jobs: {:?}. Sleeping for 30s", e);

                            let mut count = 300; // 10 * 100ms * 30s
                            while !ctx.is_done() && count > 0 {
                                thread::sleep(Duration::from_millis(100));
                                count -= 1;
                            }

                            if ctx.is_done() {
                                tracing::info!("Polling context is cancelled");
                                return;
                            }
                            continue;
                        }
                        e => {
                            poll_tx.send(Err(e)).unwrap();
                            return;
                        }
                    },
                    e => {
                        tracing::error!("Received an error {e:?}. Stopping");
                        return;
                    }
                }
            }
        }
    }
}

pub trait WorkerBuilder {
    type Worker: worker::Worker;
    fn build(&self, job: JobAcquiredResponse) -> Result<Self::Worker>;
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

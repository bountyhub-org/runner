use client::runner::{CompleteRequest, JobAcquiredResponse, PollRequest, RunnerClient};
use config::ConfigManager;
use ctx::{Background, Ctx};
use miette::{IntoDiagnostic, Result};
#[cfg(test)]
use mockall::automock;
use std::fs;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
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

#[derive(Debug, Clone)]
enum RunnerEvent {
    AcquiredJobs(Vec<JobAcquiredResponse>),
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

        let capacity = self.config_manager.get()?.capacity;
        assert!(capacity > 0, "Config capacity set to 0 doesn't make sense");

        let (event_tx, event_rx) = mpsc::sync_channel(2);
        let (worker_joiner_tx, worker_joiner_rx) = mpsc::sync_channel(1);
        let listener_ctx = ctx.clone();
        let listener_client = client.clone();
        let listener_capacity = capacity;
        thread::spawn(move || {
            poll_loop(
                listener_ctx,
                listener_client,
                event_tx,
                worker_joiner_rx,
                listener_capacity,
            )
        });

        let (worker_tx, worker_rx) = mpsc::sync_channel(capacity as usize);
        let worker_joiner_capacity = capacity;
        let worker_joiner_ctx = ctx.clone();
        let worker_joiner_handle = thread::spawn(move || {
            join_workers(
                worker_joiner_ctx,
                worker_rx,
                worker_joiner_tx,
                worker_joiner_capacity,
            )
        });

        loop {
            match event_rx.try_recv() {
                Ok(RunnerEvent::AcquiredJobs(jobs)) => {
                    for job in jobs {
                        let complete_id = job.id;
                        let job_id = job.id;

                        let worker = self.worker_builder.build(job).unwrap();
                        let worker_ctx = ctx.clone();
                        let client = client.clone();
                        let worker_handle = thread::spawn(move || {
                            if let Err(e) = worker.run(worker_ctx) {
                                tracing::error!("Worker returned an error: {e:?}");
                            };
                            if let Err(e) = client.complete(
                                ctx::background(),
                                &CompleteRequest {
                                    job_id: complete_id,
                                },
                            ) {
                                tracing::error!("Failed to complete the job {complete_id}: {e:?}");
                            }
                        });
                        worker_tx
                            .send((job_id, Some(worker_handle)))
                            .expect("to send the spawned worker");
                    }
                }
                Err(TryRecvError::Empty) => {
                    if ctx.is_done() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(500));
                    continue;
                }
                Err(_) => {
                    tracing::error!("Channel closed, exiting...");
                    break;
                }
            }
        }

        if let Err(e) = worker_joiner_handle.join() {
            tracing::error!("Error returned while joining the worker joiner handle: {e:?}");
        }

        Ok(())
    }
}

fn poll_loop<RC>(
    ctx: Ctx<Background>,
    client: RC,
    poll_tx: SyncSender<RunnerEvent>,
    joined_rx: Receiver<u32>,
    mut capacity: u32,
) -> Result<()>
where
    RC: RunnerClient,
{
    loop {
        if ctx.is_done() {
            tracing::info!("Polling context is cancelled");
            return Ok(());
        }

        if let Ok(joined) = joined_rx.try_recv() {
            capacity += joined;
        }

        let jobs = client.request(ctx.to_background(), &PollRequest { capacity })?;
        if jobs.is_empty() {
            thread::sleep(Duration::from_secs(5));
            continue;
        }

        capacity -= jobs.len() as u32;

        poll_tx
            .send(RunnerEvent::AcquiredJobs(jobs))
            .expect("to send acquired jobs");

        thread::sleep(Duration::from_secs(5));
    }
}

#[tracing::instrument(skip(ctx))]
fn join_workers(
    ctx: Ctx<Background>,
    worker_rx: Receiver<(Uuid, Option<JoinHandle<()>>)>,
    worker_joiner_tx: SyncSender<u32>,
    capacity: u32,
) {
    let mut handles = Vec::with_capacity(capacity as usize);

    loop {
        if ctx.is_done() {
            tracing::debug!("Context is done");
            return;
        }

        loop {
            match worker_rx.try_recv() {
                Ok(id_handle) => {
                    tracing::debug!("Received handle {}", id_handle.0);
                    handles.push(id_handle)
                }
                Err(TryRecvError::Empty) => {
                    tracing::debug!("Empty");
                    break;
                }
                Err(TryRecvError::Disconnected) => {
                    tracing::debug!("Channel disconnected");
                    return;
                }
            };
        }

        if handles.is_empty() {
            continue;
        }

        let mut done_count = 0;
        handles.retain_mut(|(id, handle): &mut (Uuid, Option<JoinHandle<()>>)| {
            if !handle.as_ref().unwrap().is_finished() {
                return true;
            }

            tracing::debug!("Joining job id: {id:?}");
            done_count += 1;
            if let Err(e) = handle.take().unwrap().join() {
                tracing::error!("Failed to join the worker thread: {e:?}");
            };

            false
        });

        if done_count > 0 {
            if let Err(e) = worker_joiner_tx.send(done_count) {
                tracing::error!("Failed to send done count: {e:?}");
                break;
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
    use crate::poll_loop;
    use client::runner::MockRunnerClient;

    #[test]
    fn test_poll_loop_ctx_cancelled() {
        let ctx = ctx::background().with_cancel();
        ctx.cancel();

        let rc = MockRunnerClient::new();
        let (poll_tx, _poll_rx) = mpsc::sync_channel(1);
        let (_joined_tx, joined_rx) = mpsc::sync_channel(1);

        poll_loop(ctx.to_background(), rc, poll_tx, joined_rx, 1)
            .expect("expected poll loop to exit with Ok(())");
    }

    #[test]
    fn test_join_workers_ctx_cancelled() {
        let ctx = ctx::background().with_cancel();
        ctx.cancel();

        let (_worker_tx, worker_rx) = mpsc::sync_channel(1);
        let (worker_joiner_tx, _worker_joiner_rx) = mpsc::sync_channel(1);

        panic_after(Duration::from_secs(1), move || {
            join_workers(ctx.to_background(), worker_rx, worker_joiner_tx, 1)
        });
    }

    #[test]
    fn test_join_workers_proper_count() {
        panic_after(Duration::from_secs(10),  || {
            let ctx = ctx::background();
            let ctx = ctx.with_cancel();
            let (worker_tx, worker_rx) = mpsc::sync_channel(2);
            let (worker_joiner_tx, worker_joiner_rx) = mpsc::sync_channel(2);

            let join_ctx = ctx.to_background();
            let join_handle = thread::spawn(move || {
                join_workers(join_ctx, worker_rx, worker_joiner_tx, 2);
            });

            let h1 = thread::spawn(|| thread::sleep(Duration::from_secs(1)));
            let h2 = thread::spawn(|| thread::sleep(Duration::from_millis(500)));

            worker_tx
                .send((Uuid::new_v4(), Some(h1)))
                .expect("to send first handle");
            worker_tx
                .send((Uuid::new_v4(), Some(h2)))
                .expect("to send second handle");

            thread::spawn(move || {
                thread::sleep(Duration::from_secs(5));
                ctx.cancel();
            });

            let mut done = 0;
            while let Ok(count) = worker_joiner_rx.recv() {
                done += count;
            }
            join_handle.join().expect("To join the join handle");
            assert_eq!(done, 2);
        });
    }

    fn panic_after<T, F>(d: Duration, f: F) -> T
    where
        T: Send + 'static,
        F: FnOnce() -> T,
        F: Send + 'static,
    {
        let (done_tx, done_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let val = f();
            done_tx.send(()).expect("Unable to send completion signal");
            val
        });

        match done_rx.recv_timeout(d) {
            Ok(_) => handle.join().expect("Thread panicked"),
            Err(_) => panic!("Thread took too long"),
        }
    }
}

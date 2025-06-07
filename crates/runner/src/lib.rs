use client::error::ClientError;
use client::runner::{CompleteRequest, JobMessage, PollRequest, RunnerClient};
use config::ConfigManager;
use ctx::{Background, Cancel, Ctx};
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
        let listener_ctx = ctx.clone();
        let listener_client = client.clone();
        let listener_capacity = capacity;
        thread::spawn(move || {
            poll_loop(listener_ctx, listener_client, event_tx, listener_capacity)
        });

        let worker_joiner_handle = {
            let (worker_tx, worker_rx) = mpsc::sync_channel(capacity as usize);
            let worker_joiner_capacity = capacity;
            let worker_joiner_ctx = ctx.clone();
            let worker_joiner_handle = thread::spawn(move || {
                join_workers(worker_joiner_ctx, worker_rx, worker_joiner_capacity)
            });

            loop {
                match event_rx.try_recv() {
                    Ok(messages) => {
                        for message in messages {
                            match message {
                                JobMessage::JobAssigned { id, token } => {
                                    let worker = self.worker_builder.build(id, token).unwrap();
                                    let worker_handle_ctx = ctx.with_cancel();
                                    let worker_ctx = worker_handle_ctx.to_background();
                                    let client = client.clone();
                                    let worker_handle = thread::spawn(move || {
                                        if let Err(e) = worker.run(worker_ctx) {
                                            tracing::error!("Worker returned an error: {e:?}");
                                        };
                                        if let Err(e) = client.complete(
                                            ctx::background(),
                                            &CompleteRequest { job_id: id },
                                        ) {
                                            tracing::error!(
                                                "Failed to complete the job {id}: {e:?}"
                                            );
                                        }
                                    });
                                    worker_tx
                                        .send(JoinWorkersEvent::JobStarted(JobStarted {
                                            id,
                                            ctx: worker_handle_ctx,
                                            handle: Some(worker_handle),
                                        }))
                                        .expect("to send the spawned worker");
                                }
                                JobMessage::JobCancelled { id } => {
                                    tracing::info!("Job {id} was cancelled");
                                    worker_tx
                                        .send(JoinWorkersEvent::JobCancelled(id))
                                        .expect("to send the cancelled job");
                                }
                            }
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

            worker_joiner_handle
        };

        tracing::info!("Shutting down, waiting for workers to finish...");
        if let Err(e) = worker_joiner_handle.join() {
            tracing::error!("Error returned while joining the worker joiner handle: {e:?}");
        }

        Ok(())
    }
}

fn poll_loop<RC>(
    ctx: Ctx<Background>,
    client: RC,
    poll_tx: SyncSender<Vec<JobMessage>>,
    capacity: u32,
) -> Result<()>
where
    RC: RunnerClient,
{
    loop {
        if ctx.is_done() {
            tracing::info!("Polling context is cancelled");
            return Ok(());
        }

        let req = PollRequest { capacity };
        let messages = match client.get_messages(ctx.to_background(), &req) {
            Ok(jobs) => {
                tracing::debug!("Received {} jobs", jobs.len());
                jobs
            }
            Err(e) => {
                if e.downcast_ref::<ClientError>()
                    .is_some_and(|e| e.is_retryable())
                {
                    tracing::debug!("Retrying job request on error: {e:?}");
                    thread::sleep(Duration::from_secs(1));
                    continue;
                }
                tracing::error!("Failed to poll for jobs: {e:?}");
                continue;
            }
        };

        poll_tx.send(messages).expect("to send acquired jobs");
    }
}

enum JoinWorkersEvent {
    JobStarted(JobStarted),
    JobCancelled(Uuid),
}

struct JobStarted {
    id: Uuid,
    ctx: Ctx<Cancel>,
    handle: Option<JoinHandle<()>>,
}

#[tracing::instrument(skip(ctx))]
fn join_workers(ctx: Ctx<Background>, worker_rx: Receiver<JoinWorkersEvent>, capacity: u32) {
    let mut handles: Vec<JobStarted> = Vec::with_capacity(capacity as usize);

    loop {
        if ctx.is_done() {
            tracing::debug!("Context is done");
            return;
        }

        loop {
            thread::sleep(Duration::from_millis(500));
            match worker_rx.try_recv() {
                Ok(JoinWorkersEvent::JobCancelled(id)) => {
                    tracing::debug!("Job cancelled: {id:?}");
                    let job_started = handles.iter_mut().find(|job_started| job_started.id == id);
                    if let Some(JobStarted { ctx, .. }) = job_started {
                        tracing::debug!("Cancelling job with id: {id:?}");
                        ctx.cancel();
                    }
                }
                Ok(JoinWorkersEvent::JobStarted(job_started)) => {
                    tracing::debug!("Received handle {}", job_started.id);
                    handles.push(job_started);
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
        handles.retain_mut(|job_started: &mut JobStarted| {
            if !job_started.handle.as_ref().unwrap().is_finished() {
                return true;
            }

            tracing::debug!("Joining job id: {:?}", job_started.id);
            done_count += 1;
            if let Err(e) = job_started.handle.take().unwrap().join() {
                tracing::error!("Failed to join the worker thread: {e:?}");
            };

            false
        });

        if done_count > 0 {
            tracing::debug!("Done count: {done_count}");
        }
    }
}

pub trait WorkerBuilder {
    type Worker: worker::Worker;
    fn build(&self, id: Uuid, token: String) -> Result<Self::Worker>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use client::runner::MockRunnerClient;

    #[test]
    fn test_poll_loop_ctx_cancelled() {
        let ctx = ctx::background();
        let ctx = ctx.with_cancel();
        ctx.cancel();

        let rc = MockRunnerClient::new();
        let (poll_tx, _poll_rx) = mpsc::sync_channel(1);

        poll_loop(ctx.to_background(), rc, poll_tx, 1)
            .expect("expected poll loop to exit with Ok(())");
    }

    #[test]
    fn test_poll_loop_requests_properly() {
        panic_after(Duration::from_secs(20), || {
            let ctx = ctx::background();
            let ctx = ctx.with_cancel();

            let mut rc = MockRunnerClient::new();

            let (poll_tx, poll_rx) = mpsc::sync_channel(1);

            rc.expect_get_messages()
                .returning(|_, req| {
                    assert_eq!(req.capacity, 2);
                    write_job_acquired_from_cap(2)
                })
                .once();

            // Always propagate capacity
            rc.expect_get_messages()
                .returning(|_, req| {
                    assert_eq!(req.capacity, 2);
                    write_job_acquired_from_cap(1)
                })
                .once();

            let poll_ctx = ctx.to_background();

            thread::spawn(move || {
                thread::sleep(Duration::from_secs(10));
                ctx.cancel();
            });

            thread::spawn(move || {
                poll_loop(poll_ctx, rc, poll_tx, 2)
                    .expect("expected poll loop to exit with Ok(())");
            });

            let mut acquired = 0;
            while let Ok(v) = poll_rx.recv() {
                for v in v {
                    tracing::debug!("Received job message: {:?}", v);
                    match v {
                        JobMessage::JobAssigned { .. } => {
                            acquired += 1;
                        }
                        JobMessage::JobCancelled { .. } => {
                            unreachable!("JobCancelled should not be received in this test");
                        }
                    }
                }
            }

            assert_eq!(3, acquired);
        });
    }

    #[test]
    fn test_join_workers_ctx_cancelled() {
        let ctx = ctx::background();
        let ctx = ctx.with_cancel();
        ctx.cancel();

        let (_worker_tx, worker_rx) = mpsc::sync_channel(1);

        panic_after(Duration::from_secs(1), move || {
            join_workers(ctx.to_background(), worker_rx, 1)
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

    fn write_job_acquired_from_cap(cap: usize) -> Result<Vec<JobMessage>> {
        let mut result = Vec::with_capacity(cap);
        for _ in 0..cap {
            result.push(JobMessage::JobAssigned {
                id: Uuid::now_v7(),
                token: Uuid::new_v4().to_string(),
            });
        }
        Ok(result)
    }
}

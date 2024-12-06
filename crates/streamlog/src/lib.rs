use client::worker::WorkerClient;
use ctx::{Background, Ctx};
use std::{io::Write, sync::mpsc::Receiver, thread};
use time::OffsetDateTime;
use uuid::Uuid;

pub struct Stream<C>
where
    C: WorkerClient,
{
    pub client: C,
    pub step_id: Uuid,
    pub job_id: Uuid,
    pub project_id: Uuid,
    pub workflow_id: Uuid,
    pub revision_id: Uuid,
}

#[derive(Debug, Clone, Copy)]
pub enum LogDestination {
    Stdout = 1,
    Stderr = 2,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub dst: LogDestination,
    pub timestamp: OffsetDateTime,
    pub message: String,
}

impl<C> Stream<C>
where
    C: WorkerClient,
{
    #[tracing::instrument(skip(self, ctx, rx))]
    pub fn stream(&self, ctx: Ctx<Background>, rx: Receiver<LogLine>) {
        let (mut reader, mut writer) = pipe::pipe();

        let handle = thread::spawn(move || {
            while let Ok(log_line) = rx.recv() {
                tracing::debug!("Received log line: {:?}", log_line);
                let lines = log_line.message.split('\n');
                let prefix = format!(
                    "[{}][{}]",
                    log_line.dst as i32,
                    log_line.timestamp.unix_timestamp() * 1000
                        + log_line.timestamp.millisecond() as i64
                );
                for line in lines {
                    writer
                        .write_all(format!("{}{}\n", prefix, line).as_bytes())
                        .unwrap_or_else(|err| tracing::error!("Error writing to pipe: {:?}", err));
                }
            }
        });

        self.client
            .stream_job_step_log(
                ctx,
                self.project_id,
                self.workflow_id,
                self.revision_id,
                self.job_id,
                self.step_id,
                &mut reader,
            )
            .unwrap_or_else(|err| tracing::error!("Error streaming job log: {}", err));

        handle.join().unwrap();
    }
}

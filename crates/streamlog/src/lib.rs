use client::job::{JobClient, StepRef};
use ctx::{Background, Ctx};
use std::{io::Write, sync::mpsc::Receiver, thread};
use time::OffsetDateTime;

pub struct Stream<C>
where
    C: JobClient,
{
    pub client: C,
    pub step_ref: StepRef,
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
    C: JobClient,
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
            .stream_job_step_log(ctx, &self.step_ref, &mut reader)
            .unwrap_or_else(|err| tracing::error!("Error streaming job log: {}", err));

        handle.join().unwrap();
    }
}

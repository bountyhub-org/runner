use client::invoker::{LogLine, WorkerRequestEvent};
use miette::{bail, IntoDiagnostic, Result};
use tokio::fs;
use tokio::sync::mpsc::Sender;

pub(crate) trait Step {
    async fn run(&self, tx: Sender<WorkerRequestEvent>) -> Result<()>;
}

pub(crate) struct SetupStep<'a> {
    index: u32,
    workdir: &'a str,
}

impl Step for SetupStep<'_> {
    async fn run(&self, tx: Sender<WorkerRequestEvent>) -> Result<()> {
        match fs::create_dir_all(self.workdir).await {
            Ok(_) => {
                tx.send(WorkerRequestEvent::SendLogLine {
                    line: LogLine::stdout(
                        self.index,
                        format!("Created workdir: '{}'", self.workdir),
                    ),
                })
                .await
                .into_diagnostic()?;
            }
            Err(e) => {
                tx.send(WorkerRequestEvent::SendLogLine {
                    line: LogLine::stderr(
                        self.index,
                        format!("Failed to create workdir '{}': {}", self.workdir, e),
                    ),
                })
                .await
                .into_diagnostic()?;
                bail!(e);
            }
        };

        todo!()
    }
}

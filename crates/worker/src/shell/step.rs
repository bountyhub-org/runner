use client::invoker::{LogLine, StepState, WorkerRequestEvent};
use miette::{Context, IntoDiagnostic, Result, WrapErr};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;
use uuid::Uuid;

use super::execution_context::ExecutionContext;

pub(crate) trait Step {
    async fn run(&self, tx: Sender<WorkerRequestEvent>) -> Result<bool>;
}

#[derive(Debug)]
pub(crate) struct SetupStep {
    index: u32,
    context: Arc<ExecutionContext>,
}

impl Step for SetupStep {
    #[tracing::instrument(skip(tx))]
    async fn run(&self, tx: Sender<WorkerRequestEvent>) -> Result<bool> {
        let workdir = self.context.workdir();

        tracing::debug!("Send setup step running");
        tx.send(WorkerRequestEvent::StepTimeline {
            step_index: self.index,
            state: StepState::Running,
        })
        .await
        .into_diagnostic()?;

        tracing::debug!("Creting workdir '{workdir}'");
        match fs::create_dir_all(workdir).await {
            Ok(_) => {
                tracing::debug!("Successfully created workdir '{workdir}'");
                tx.send(WorkerRequestEvent::SendLogLine {
                    line: LogLine::stdout(self.index, format!("Created workdir: '{workdir}'")),
                })
                .await
                .into_diagnostic()
                .wrap_err("Failed to send stdout log")?;

                tx.send(WorkerRequestEvent::StepTimeline {
                    step_index: self.index,
                    state: StepState::Succeeded,
                })
                .await
                .into_diagnostic()
                .wrap_err("Failed to send succeeded step timeline")?;

                Ok(true)
            }
            Err(e) => {
                tracing::error!("Failed to create workdir: '{workdir}'");
                tx.send(WorkerRequestEvent::SendLogLine {
                    line: LogLine::stderr(
                        self.index,
                        format!("Failed to create workdir '{workdir}': {e:?}"),
                    ),
                })
                .await
                .into_diagnostic()?;

                tx.send(WorkerRequestEvent::StepTimeline {
                    step_index: self.index,
                    state: StepState::Succeeded,
                })
                .await
                .into_diagnostic()
                .wrap_err("Failed to send failed step timeline")?;

                Ok(false)
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct TeardownStep {
    index: u32,
    context: Arc<ExecutionContext>,
}

impl Step for TeardownStep {
    #[tracing::instrument(skip(tx))]
    async fn run(&self, tx: Sender<WorkerRequestEvent>) -> Result<bool> {
        let workdir = self.context.workdir();

        tracing::debug!("Send teardown step running");
        tx.send(WorkerRequestEvent::StepTimeline {
            step_index: self.index,
            state: StepState::Running,
        })
        .await
        .into_diagnostic()?;

        tracing::debug!("Removing workdir '{workdir}'");
        match fs::remove_dir_all(workdir).await {
            Ok(_) => {
                tracing::debug!("Successfully removed workdir '{workdir}'");
                tx.send(WorkerRequestEvent::SendLogLine {
                    line: LogLine::stdout(self.index, format!("Removed workdir: '{workdir}'")),
                })
                .await
                .into_diagnostic()
                .wrap_err("Failed to send stdout log")?;

                tx.send(WorkerRequestEvent::StepTimeline {
                    step_index: self.index,
                    state: StepState::Succeeded,
                })
                .await
                .into_diagnostic()
                .wrap_err("Failed to send succeeded step timeline")?;

                Ok(true)
            }
            Err(e) => {
                tracing::error!("Failed to remove workdir: '{workdir}'");
                tx.send(WorkerRequestEvent::SendLogLine {
                    line: LogLine::stderr(
                        self.index,
                        format!("Failed to remove workdir '{workdir}': {e:?}"),
                    ),
                })
                .await
                .into_diagnostic()?;

                tx.send(WorkerRequestEvent::StepTimeline {
                    step_index: self.index,
                    state: StepState::Failed,
                })
                .await
                .into_diagnostic()
                .wrap_err("Failed to send failed step timeline")?;

                Ok(false)
            }
        }
    }
}

#[derive(Debug)]
struct CommandStep<'a> {
    index: u32,
    context: Arc<ExecutionContext>,
    run: &'a str,
    shell: &'a str,
    allow_failed: bool,
}

impl Step for CommandStep<'_> {
    #[tracing::instrument(skip(tx))]
    async fn run(&self, tx: Sender<WorkerRequestEvent>) -> Result<bool> {
        tracing::debug!("Sending command step running");
        tx.send(WorkerRequestEvent::StepTimeline {
            step_index: self.index,
            state: StepState::Running,
        })
        .await
        .into_diagnostic()
        .wrap_err("Failed to post step running")?;

        tracing::debug!("Shell split: {}", self.shell);
        let mut cmd = match shlex::split(self.shell) {
            Some(cmd) => cmd,
            None => {
                tx.send(WorkerRequestEvent::SendLogLine {
                    line: LogLine::stderr(
                        self.index,
                        format!("Shell split failed for shell: '{}'", self.shell),
                    ),
                })
                .await
                .into_diagnostic()
                .wrap_err("Failed to post log timeline")?;

                tx.send(WorkerRequestEvent::StepTimeline {
                    step_index: self.index,
                    state: StepState::Failed,
                })
                .await
                .into_diagnostic()
                .wrap_err("Failed to send failed step timeline")?;

                return Ok(self.allow_failed);
            }
        };

        tracing::debug!("Writing script");
        let script_path = match self.write_script().await {
            Ok(path) => path,
            Err(e) => {
                tx.send(WorkerRequestEvent::SendLogLine {
                    line: LogLine::stderr(self.index, format!("Failed to write script: {e:?}")),
                })
                .await
                .into_diagnostic()
                .wrap_err("Failed to send the log line")?;

                tx.send(WorkerRequestEvent::StepTimeline {
                    step_index: self.index,
                    state: StepState::Failed,
                })
                .await
                .into_diagnostic()
                .wrap_err("Failed to send failed step timeline")?;

                return Ok(self.allow_failed);
            }
        };

        cmd.push(
            script_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        );

        tracing::debug!("Command: '{}'", cmd.join(" "));

        let bin = cmd.remove(0);
        let args = cmd;
        let mut cmd = Command::new(bin);
        cmd.args(args);
        cmd.current_dir(self.context.workdir());
        cmd.envs(self.context.envs().iter().cloned());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .into_diagnostic()
            .wrap_err("Failed to spawn the command")?;

        let mut stdout =
            BufReader::new(child.stdout.take().expect("Failed to open stdout")).lines();
        let mut stderr =
            BufReader::new(child.stderr.take().expect("Failed to open stderr")).lines();

        let index = self.index;
        let stdout_tx = tx.clone();
        let stdout: JoinHandle<Result<()>> = tokio::spawn(async move {
            while let Some(line) = stdout.next_line().await.into_diagnostic()? {
                stdout_tx
                    .send(WorkerRequestEvent::SendLogLine {
                        line: LogLine::stdout(index, line),
                    })
                    .await
                    .into_diagnostic()
                    .wrap_err("Failed to send stdout log line")?;
            }
            Ok(())
        });

        let stderr_tx = tx.clone();
        let stderr: JoinHandle<Result<()>> = tokio::spawn(async move {
            while let Some(line) = stderr
                .next_line()
                .await
                .into_diagnostic()
                .wrap_err("Failed to read next line from stderr")?
            {
                stderr_tx
                    .send(WorkerRequestEvent::SendLogLine {
                        line: LogLine::stderr(index, line),
                    })
                    .await
                    .into_diagnostic()
                    .wrap_err("Failed to send stderr log line")?;
            }
            Ok(())
        });

        let run = tokio::spawn(async move {
            child
                .wait()
                .await
                .into_diagnostic()
                .wrap_err("Failed to wait for child process")
        });

        let (run, stdout, stderr) = tokio::join!(run, stdout, stderr);

        let exit_status = run
            .into_diagnostic()?
            .wrap_err("Failed to await exit status")?;

        let exit_code = exit_status.code().unwrap_or(1);
        stdout
            .into_diagnostic()?
            .wrap_err("Stdout returned an error")?;

        stderr
            .into_diagnostic()?
            .wrap_err("Stderr returned an error")?;

        Ok(exit_code == 0 || self.allow_failed)
    }
}

impl CommandStep<'_> {
    #[tracing::instrument(skip(self))]
    async fn write_script(&self) -> Result<PathBuf> {
        tracing::debug!("Writing script");
        tracing::debug!("Evaluating code template");
        let script = self
            .context
            .eval_templ(self.run)
            .wrap_err("Failed to evaluate code template")?;

        let file_path = Path::new(self.context.workdir()).join(Uuid::new_v4().to_string());

        fs::write(&file_path, script)
            .await
            .into_diagnostic()
            .wrap_err("Failed to write file")?;

        Ok(file_path)
    }
}

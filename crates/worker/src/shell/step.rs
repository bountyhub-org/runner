use cellang::Value;
use client::invoker::{LogLine, StepState, WorkerRequestEvent};
use miette::{bail, Context, IntoDiagnostic, Result};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::fs::{self};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::Sender;
use tokio::task::JoinHandle;
use uuid::Uuid;
use zip::write::{FileOptionExtension, FileOptions, SimpleFileOptions};
use zip::ZipWriter;

use super::execution_context::ExecutionContext;

pub(crate) trait RunStep {
    async fn run(&self, tx: Sender<WorkerRequestEvent>) -> Result<bool>;
}

#[derive(Debug)]
pub(crate) struct SetupStep<'a> {
    pub(crate) index: u32,
    pub(crate) context: &'a ExecutionContext,
}

impl RunStep for SetupStep<'_> {
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
pub(crate) struct TeardownStep<'a> {
    pub(crate) index: u32,
    pub(crate) context: &'a ExecutionContext,
}

impl RunStep for TeardownStep<'_> {
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
pub(crate) struct CommandStep<'a> {
    pub(crate) index: u32,
    pub(crate) context: &'a ExecutionContext,
    pub(crate) cond: &'a str,
    pub(crate) run: &'a str,
    pub(crate) shell: &'a str,
    pub(crate) allow_failed: bool,
}

impl RunStep for CommandStep<'_> {
    #[tracing::instrument(skip(tx))]
    async fn run(&self, tx: Sender<WorkerRequestEvent>) -> Result<bool> {
        match self
            .context
            .eval_expr(self.cond)
            .wrap_err("Condition evaluation failed")?
        {
            Value::Bool(false) => {
                tx.send(WorkerRequestEvent::StepTimeline {
                    step_index: self.index,
                    state: StepState::Skipped,
                })
                .await
                .into_diagnostic()
                .wrap_err("Failed to post step skipped")?;
                return Ok(true);
            }
            Value::Bool(true) => {}
            v => bail!("Condition evaluated to value {v:?}, expected bool"),
        };

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

        if exit_code == 0 {
            tx.send(WorkerRequestEvent::StepTimeline {
                step_index: self.index,
                state: StepState::Succeeded,
            })
            .await
            .into_diagnostic()
            .wrap_err("Failed to write succeeded step timeline")?;

            Ok(true)
        } else {
            tx.send(WorkerRequestEvent::StepTimeline {
                step_index: self.index,
                state: StepState::Failed,
            })
            .await
            .into_diagnostic()
            .wrap_err("Failed to write failed step timeline")?;

            Ok(self.allow_failed)
        }
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

#[derive(Debug)]
pub(crate) struct UploadStep<'a> {
    pub(crate) index: u32,
    pub(crate) context: &'a ExecutionContext,
    pub(crate) uploads: &'a Vec<String>,
}

impl RunStep for UploadStep<'_> {
    #[tracing::instrument(skip(tx))]
    async fn run(&self, tx: Sender<WorkerRequestEvent>) -> Result<bool> {
        if !self.context.ok() {
            tx.send(WorkerRequestEvent::StepTimeline {
                step_index: self.index,
                state: StepState::Skipped,
            })
            .await
            .into_diagnostic()
            .wrap_err("Failed to post step skipped")?;
            return Ok(true);
        }

        tx.send(WorkerRequestEvent::StepTimeline {
            step_index: self.index,
            state: StepState::Running,
        })
        .await
        .into_diagnostic()
        .wrap_err("Failed to post step running")?;

        let workdir = PathBuf::from(self.context.workdir());

        let filename = format!("{}.zip", Uuid::new_v4());
        let result_path = workdir.join(filename);

        let uploads = self.uploads.clone();
        let result: JoinHandle<Result<()>> = tokio::task::spawn_blocking(move || {
            let result = std::fs::File::create(result_path.clone()).into_diagnostic()?;
            let mut zip = ZipWriter::new(result);

            let option = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .compression_level(Some(9));

            for upload in uploads {
                let path = normalize_abs_path(&workdir, Path::new(upload.as_str()))
                    .wrap_err("Failed to normalize path")?;

                if path.is_dir() {
                    add_dir_to_zip(&mut zip, &path, &workdir, option)?;
                } else {
                    add_file_to_zip(&mut zip, &path, &path, option)?;
                }
            }

            Ok(())
        });

        result.await.into_diagnostic()?.wrap_err("Zip failed")?;

        Ok(true)
    }
}

#[tracing::instrument]
fn normalize_abs_path(root: &Path, s: &Path) -> Result<PathBuf> {
    let mut components = s.components().peekable();
    let mut ret = if let Some(c @ Component::Prefix(..)) = components.peek().cloned() {
        components.next();
        PathBuf::from(c.as_os_str())
    } else {
        PathBuf::new()
    };

    for component in components {
        match component {
            Component::Prefix(..) => unreachable!(),
            Component::RootDir => {
                bail!("upload record {s:?} cannot be taken from the root");
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !ret.pop() {
                    bail!("upload record {s:?} cannot be outside of the present working directory");
                }
            }
            Component::Normal(c) => {
                ret.push(c);
            }
        }
    }

    // double check when symlinks are resolved
    let canonized = root
        .join(ret)
        .canonicalize()
        .into_diagnostic()
        .wrap_err("failed to canonicalize path")?;

    if !canonized.starts_with(root) {
        bail!("path '{s:?}' after resolution to '{canonized:?}' doesn't start with {root:?}",)
    }

    Ok(canonized)
}

#[tracing::instrument(skip(w, option))]
fn add_file_to_zip<T>(
    w: &mut ZipWriter<std::fs::File>,
    src: &Path,
    dst: &Path,
    option: FileOptions<T>,
) -> Result<()>
where
    T: FileOptionExtension + Copy,
{
    w.start_file_from_path(dst, option)
        .into_diagnostic()
        .wrap_err("Failed to start file from path")?;

    let mut f = std::fs::File::open(src)
        .into_diagnostic()
        .wrap_err("Failed to open file to zip")?;

    std::io::copy(&mut f, w)
        .into_diagnostic()
        .wrap_err("Failed to copy content from file to zip")?;

    Ok(())
}

#[tracing::instrument(skip(w, option))]
fn add_dir_to_zip<T>(
    w: &mut ZipWriter<std::fs::File>,
    dir_path: &Path,
    base_path: &Path,
    option: FileOptions<T>,
) -> Result<()>
where
    T: FileOptionExtension + Copy,
{
    for entry in std::fs::read_dir(dir_path)
        .into_diagnostic()
        .wrap_err("Failed to read directory")?
    {
        let path = entry.into_diagnostic()?.path();
        if path.is_symlink() {
            tracing::info!("Path is symlink, skipping");
            continue;
        }

        let relative_path = path
            .strip_prefix(base_path)
            .into_diagnostic()
            .wrap_err("Failed to create relative path")?;

        if path.is_dir() {
            add_dir_to_zip(w, &path, base_path, option)?;
        } else {
            add_file_to_zip(w, &path, relative_path, option)?;
        }
    }
    Ok(())
}

use super::execution_context::ExecutionContext;
use cellang::Value;
use client::job::{JobClient, TimelineRequest, TimelineRequestStepOutcome};
use client::job::{LogLine, TimelineRequestStepState};
use ctx::{Background, Ctx};
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use uuid::Uuid;
use zip::write::{FileOptionExtension, FileOptions, SimpleFileOptions};
use zip::ZipWriter;

pub trait Step {
    fn run(&self, ctx: Ctx<Background>) -> Result<bool>;
}

#[derive(Debug, Clone)]
pub struct SetupStep<'a, C>
where
    C: JobClient,
{
    pub index: u32,
    pub context: &'a ExecutionContext,
    pub worker_client: C,
}

impl<C> Step for SetupStep<'_, C>
where
    C: JobClient,
{
    #[tracing::instrument(skip(self, ctx))]
    fn run(&self, ctx: Ctx<Background>) -> Result<bool> {
        tracing::debug!("Executing setup step");
        let workdir = self.context.job_dir();

        let timeline_request = TimelineRequest {
            index: self.index,
            state: TimelineRequestStepState::Running,
        };
        tracing::debug!("Posting step state: {timeline_request:?}");
        self.worker_client
            .post_step_timeline(ctx.clone(), &timeline_request)
            .wrap_err("Failed to post step timeline")?;

        tracing::debug!("Creating job workdir {workdir:?}");
        match fs::create_dir_all(workdir) {
            Ok(_) => {
                let msg = format!("Sucessfully created job workdir {workdir:?}");
                tracing::debug!("{msg}");
                self.worker_client
                    .send_job_logs(ctx.clone(), &[LogLine::stdout(self.index, &msg)])
                    .wrap_err("Failed to send logs")?;

                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: TimelineRequestStepState::Succeeded,
                };
                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;
                tracing::debug!("Posted step state: {timeline_request:?}");

                tracing::debug!("Posted setup step");
                Ok(true)
            }
            Err(e) => {
                let msg = format!("Failed to create job workdir {workdir:?}: {e:?}");
                tracing::debug!("{msg}");
                self.worker_client
                    .send_job_logs(ctx.clone(), &[LogLine::stderr(self.index, &msg)])
                    .wrap_err("Failed to send logs")?;

                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: TimelineRequestStepState::Failed {
                        outcome: TimelineRequestStepOutcome::Failed,
                    },
                };

                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;
                tracing::debug!("Posted step state: {timeline_request:?}");

                tracing::debug!("Posted setup step");
                Ok(false)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TeardownStep<'a, C>
where
    C: JobClient,
{
    pub index: u32,
    pub context: &'a ExecutionContext,
    pub worker_client: C,
}

impl<C> Step for TeardownStep<'_, C>
where
    C: JobClient,
{
    #[tracing::instrument(skip(self, ctx))]
    fn run(&self, ctx: Ctx<Background>) -> Result<bool> {
        tracing::debug!("Executing teardown step");
        let workdir = self.context.job_dir();

        let timeline_request = TimelineRequest {
            index: self.index,
            state: TimelineRequestStepState::Running,
        };
        tracing::debug!("Posting step state: {timeline_request:?}");
        self.worker_client
            .post_step_timeline(ctx.clone(), &timeline_request)
            .wrap_err("Failed to post step timeline")?;

        tracing::debug!("Creating job workdir {workdir:?}");
        match fs::remove_dir_all(workdir) {
            Ok(_) => {
                let msg = format!("Sucessfully removed job workdir {workdir:?}");
                tracing::debug!("{msg}");
                self.worker_client
                    .send_job_logs(ctx.clone(), &[LogLine::stdout(self.index, &msg)])
                    .wrap_err("Failed to send logs")?;

                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: TimelineRequestStepState::Succeeded,
                };
                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;
                tracing::debug!("Posted step state: {timeline_request:?}");

                tracing::debug!("Posted setup step");
                Ok(true)
            }
            Err(e) => {
                let msg = format!("Failed to remove job workdir {workdir:?}: {e:?}");
                tracing::debug!("{msg}");
                self.worker_client
                    .send_job_logs(ctx.clone(), &[LogLine::stderr(self.index, &msg)])
                    .wrap_err("Failed to send logs")?;

                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: TimelineRequestStepState::Failed {
                        outcome: TimelineRequestStepOutcome::Failed,
                    },
                };

                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;
                tracing::debug!("Posted step state: {timeline_request:?}");

                tracing::debug!("Posted setup step");
                Ok(false)
            }
        }
    }
}

#[derive(Debug)]
pub struct CommandStep<'a, C> {
    pub index: u32,
    pub context: &'a ExecutionContext,
    pub worker_client: C,
    pub cond: &'a str,
    pub run: &'a str,
    pub shell: &'a str,
    pub allow_failed: bool,
}

impl<C> Step for CommandStep<'_, C>
where
    C: JobClient,
{
    fn run(&self, ctx: Ctx<Background>) -> Result<bool> {
        if !self
            .should_run(ctx.clone())
            .wrap_err("Eval condition succeeded")?
        {
            // even if shell is false, ok should be true
            return Ok(true);
        }

        let mut cmd = self
            .split_shell(ctx.clone())
            .wrap_err("Failed to split the shell")?;

        let script_path = self
            .write_scipt(ctx.clone())
            .wrap_err("Failed to write script")?;

        cmd.push(
            script_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        );

        let binary = cmd.remove(0);
        let args = cmd;
        let mut cmd = Command::new(binary);
        cmd.args(args);
        cmd.current_dir(self.context.job_dir());
        cmd.envs(self.context.envs().iter().cloned());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        tracing::debug!("Spawning command: {cmd:?}");
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                tracing::error!("Failed to spawn child process: {err:?}");
                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: self.soft_fail_state(),
                };

                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;

                return Ok(self.allow_failed);
            }
        };

        let stdout = BufReader::new(child.stdout.take().expect("Failed to open stdout")).lines();
        let stderr = BufReader::new(child.stderr.take().expect("Failed to open stderr")).lines();

        let (log_tx, log_rx) = mpsc::sync_channel(100);

        let stdout_tx = log_tx.clone();
        let index = self.index;
        let stdout_handle = thread::spawn(move || read_and_send_line(index, stdout_tx, stdout));

        let stderr_tx = log_tx.clone();
        let index = self.index;
        let stderr_handle = thread::spawn(move || read_and_send_line(index, stderr_tx, stderr));

        let log_ctx = ctx.clone();
        let worker_client = self.worker_client.clone();
        let log_pusher: JoinHandle<Result<()>> = thread::spawn(move || {
            let mut buf = Vec::with_capacity(100);

            let mut done = false;
            while !done {
                buf.clear();
                for _ in 0..100 {
                    match log_rx.try_recv() {
                        Ok(line) => buf.push(line),
                        Err(TryRecvError::Empty) => {
                            done = true;
                            break;
                        }
                        Err(TryRecvError::Disconnected) => return Ok(()),
                    }
                }

                if buf.is_empty() {
                    thread::sleep(Duration::from_secs(1));
                    continue;
                }

                worker_client
                    .send_job_logs(log_ctx.clone(), &buf)
                    .wrap_err("Failed to send job log")?;

                thread::sleep(Duration::from_secs(1));
            }

            Ok(())
        });

        tracing::info!("Waiting for command to finish");
        let index = self.index;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if !ctx.is_done() {
                        thread::sleep(Duration::from_millis(250));
                        continue;
                    }
                    log_tx
                        .send(LogLine::stderr(
                            index,
                            "Received cancellation signal. Killing child process",
                        ))
                        .into_diagnostic()
                        .wrap_err("Failed to send the log line")?;
                    tracing::info!("Received cancellation signal. Killing child process");
                    if let Err(err) = child.kill() {
                        tracing::error!("Failed to kill child process: {}. Command might still execute in the background", err);
                    } else {
                        tracing::info!("Killed child process");
                    }

                    let timeline_request = TimelineRequest {
                        index: self.index,
                        state: TimelineRequestStepState::Skipped,
                    };
                    tracing::debug!("Posting step state: {timeline_request:?}");
                    self.worker_client
                        .post_step_timeline(ctx.clone(), &timeline_request)
                        .wrap_err("Failed to post step timeline")?;

                    return Ok(false);
                }
                Err(e) => {
                    let msg = format!("Failed to wait for child process: {e:?}");
                    tracing::error!("{msg}");
                    log_tx
                        .send(LogLine::stderr(index, &msg))
                        .into_diagnostic()
                        .wrap_err("Failed to send the stderr line")?;

                    let timeline_request = TimelineRequest {
                        index: self.index,
                        state: self.soft_fail_state(),
                    };

                    tracing::debug!("Posting step state: {timeline_request:?}");
                    self.worker_client
                        .post_step_timeline(ctx.clone(), &timeline_request)
                        .wrap_err("Failed to post step timeline")?;

                    return Ok(self.allow_failed);
                }
            }
        }

        let ok = match child.wait() {
            Ok(out) => {
                let code = out.code().unwrap_or(1);
                let ok = code == 0;
                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: if ok {
                        TimelineRequestStepState::Succeeded
                    } else {
                        self.soft_fail_state()
                    },
                };

                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;

                ok || self.allow_failed
            }
            Err(e) => {
                tracing::error!("Failed to wait child process: {e:?}");
                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: self.soft_fail_state(),
                };

                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;

                self.allow_failed
            }
        };

        tracing::debug!("Waiting stdout handle to be joined");
        if let Err(e) = stdout_handle.join().expect("Failed to join stdout handle") {
            tracing::error!("Stdout handle returned an error, trying to move on: {e:?}");
        };

        tracing::debug!("Waiting stderr handle to be joined");
        if let Err(e) = stderr_handle.join().expect("Failed to join stderr handle") {
            tracing::error!("Stderr handle returned an error, trying to move on: {e:?}");
        };

        tracing::debug!("Waiting log pusher handle to be joined");
        if let Err(e) = log_pusher.join().expect("Failed to join log pusher handle") {
            tracing::error!("Pushing logs returned an error, trying to move on: {e:?}");
        };

        Ok(ok)
    }
}

impl<C> CommandStep<'_, C>
where
    C: JobClient,
{
    #[tracing::instrument(skip(self, ctx))]
    fn should_run(&self, ctx: Ctx<Background>) -> Result<bool> {
        tracing::debug!("Testing the condition");
        match self.context.eval_expr(self.cond) {
            Ok(Value::Bool(false)) => {
                tracing::debug!("Condition evaluated to false");

                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: TimelineRequestStepState::Skipped,
                };
                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;

                Ok(false)
            }
            Ok(Value::Bool(true)) => {
                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: TimelineRequestStepState::Running,
                };
                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;

                Ok(true)
            }
            Ok(v) => {
                let msg = format!(
                    "Condition should evaluate to boolean, got {v:?}; cond '{}'",
                    self.cond
                );
                tracing::debug!("{msg}");
                self.worker_client
                    .send_job_logs(ctx.clone(), &[LogLine::stderr(self.index, &msg)])
                    .wrap_err("Failed to send logs")?;

                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: TimelineRequestStepState::Failed {
                        outcome: TimelineRequestStepOutcome::Failed,
                    },
                };
                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;

                bail!("Condition evaluated to value {v:?}, expected bool")
            }
            Err(e) => {
                let msg = format!(
                    "Failed to evaluate the if condition '{}': '{e:?}'",
                    self.cond
                );
                tracing::debug!("{msg}");
                self.worker_client
                    .send_job_logs(ctx.clone(), &[LogLine::stderr(self.index, &msg)])
                    .wrap_err("Failed to send logs")?;

                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: TimelineRequestStepState::Failed {
                        outcome: TimelineRequestStepOutcome::Failed,
                    },
                };
                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;

                bail!("Condition evaluated to value {e:?}, expected bool")
            }
        }
    }

    #[tracing::instrument(skip(self, ctx))]
    fn split_shell(&self, ctx: Ctx<Background>) -> Result<Vec<String>> {
        tracing::debug!("Shell split: {}", self.shell);
        match shlex::split(self.shell) {
            Some(cmd) if !cmd.is_empty() => Ok(cmd),
            Some(_) => {
                let msg = "Shell split is empty".to_string();
                tracing::debug!("{msg}");
                self.worker_client
                    .send_job_logs(ctx.clone(), &[LogLine::stderr(self.index, &msg)])
                    .wrap_err("Failed to send logs")?;

                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: TimelineRequestStepState::Failed {
                        outcome: TimelineRequestStepOutcome::Failed,
                    },
                };
                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;

                bail!(msg)
            }
            None => {
                let msg = format!("Failed to split the shell {}", self.shell);
                tracing::debug!("{msg}");
                self.worker_client
                    .send_job_logs(ctx.clone(), &[LogLine::stderr(self.index, &msg)])
                    .wrap_err("Failed to send logs")?;

                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: TimelineRequestStepState::Failed {
                        outcome: TimelineRequestStepOutcome::Failed,
                    },
                };
                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;

                bail!(msg)
            }
        }
    }

    #[tracing::instrument(skip(self, ctx))]
    fn write_scipt(&self, ctx: Ctx<Background>) -> Result<PathBuf> {
        tracing::debug!("Writing script");

        tracing::debug!("Evaluating code template");
        let script = match self.context.eval_templ(self.run) {
            Ok(s) => s,
            Err(e) => {
                let msg = format!("Failed to evaluate the template: {e:?}");
                tracing::debug!("{msg}");
                self.worker_client
                    .send_job_logs(ctx.clone(), &[LogLine::stderr(self.index, &msg)])
                    .wrap_err("Failed to send logs")?;

                let timeline_request = TimelineRequest {
                    index: self.index,
                    state: TimelineRequestStepState::Failed {
                        outcome: TimelineRequestStepOutcome::Failed,
                    },
                };
                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;

                bail!(msg)
            }
        };

        let file_path = Path::new(self.context.job_dir()).join(Uuid::new_v4().to_string());

        if let Err(e) = fs::write(&file_path, script) {
            let msg = format!("Failed to create script file: {e:?}");
            tracing::debug!("{msg}");
            self.worker_client
                .send_job_logs(ctx.clone(), &[LogLine::stderr(self.index, &msg)])
                .wrap_err("Failed to send logs")?;

            let timeline_request = TimelineRequest {
                index: self.index,
                state: TimelineRequestStepState::Failed {
                    outcome: TimelineRequestStepOutcome::Failed,
                },
            };
            tracing::debug!("Posting step state: {timeline_request:?}");
            self.worker_client
                .post_step_timeline(ctx.clone(), &timeline_request)
                .wrap_err("Failed to post step timeline")?;

            bail!(msg)
        }

        Ok(file_path)
    }

    fn soft_fail_state(&self) -> TimelineRequestStepState {
        if self.allow_failed {
            TimelineRequestStepState::Failed {
                outcome: TimelineRequestStepOutcome::Succeeded,
            }
        } else {
            TimelineRequestStepState::Failed {
                outcome: TimelineRequestStepOutcome::Failed,
            }
        }
    }
}

#[derive(Debug)]
pub struct UploadStep<'a, C>
where
    C: JobClient,
{
    pub index: u32,
    pub context: &'a ExecutionContext,
    pub uploads: &'a Vec<String>,
    pub worker_client: C,
}

impl<C> Step for UploadStep<'_, C>
where
    C: JobClient,
{
    #[tracing::instrument(skip(self, ctx))]
    fn run(&self, ctx: Ctx<Background>) -> Result<bool> {
        if !self.context.ok() {
            tracing::debug!("Skipping upload step");

            let timeline_request = TimelineRequest {
                index: self.index,
                state: TimelineRequestStepState::Skipped,
            };
            tracing::debug!("Posting step state: {timeline_request:?}");
            self.worker_client
                .post_step_timeline(ctx.clone(), &timeline_request)
                .wrap_err("Failed to post step timeline")?;

            return Ok(true);
        }

        let timeline_request = TimelineRequest {
            index: self.index,
            state: TimelineRequestStepState::Running,
        };
        tracing::debug!("Posting step state: {timeline_request:?}");
        self.worker_client
            .post_step_timeline(ctx.clone(), &timeline_request)
            .wrap_err("Failed to post step timeline")?;

        let path_buf = match self.create_zip_file() {
            Ok(path) => path,
            Err(e) => {
                let msg = format!("Failed to create zip file: {e:?}");
                return self.fail_with_message(ctx.clone(), &msg);
            }
        };

        let file = match File::open(&path_buf) {
            Ok(file) => file,
            Err(e) => {
                let msg = format!("Failed to open file after zipping: {e:?}");
                return self.fail_with_message(ctx.clone(), &msg);
            }
        };

        match self.worker_client.upload_job_artifact(ctx.clone(), file) {
            Ok(_) => Ok(true),
            Err(e) => {
                let msg = format!("Failed to upload job artifact: {e:?}");
                return self.fail_with_message(ctx.clone(), &msg);
            }
        }
    }
}

impl<C> UploadStep<'_, C>
where
    C: JobClient,
{
    fn create_zip_file(&self) -> Result<PathBuf> {
        let workdir = PathBuf::from(self.context.job_dir());
        let result_path = workdir.join(format!("{}.zip", Uuid::new_v4()));

        let result = File::create(result_path.clone()).into_diagnostic()?;
        let mut zip = ZipWriter::new(result);

        let option = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(9));

        let workdir = workdir
            .canonicalize()
            .into_diagnostic()
            .wrap_err("Failed to canonicalize the workdir")?;

        for upload in self.uploads {
            let path = normalize_abs_path(&workdir, Path::new(upload.as_str())).wrap_err(
                format!("Failed to normalize path: workdir={workdir:?}, path={upload}"),
            )?;

            if path.is_dir() {
                add_dir_to_zip(&mut zip, &path, &workdir, option)?;
            } else {
                add_file_to_zip(
                    &mut zip,
                    &path,
                    path.strip_prefix(&workdir)
                        .into_diagnostic()
                        .wrap_err("Failed to strip workdir prefix when calculating destination")?,
                    option,
                )?;
            }
        }

        Ok(result_path)
    }

    fn fail_with_message(&self, ctx: Ctx<Background>, msg: &str) -> Result<bool> {
        tracing::debug!("{msg}");
        self.worker_client
            .send_job_logs(ctx.clone(), &[LogLine::stderr(self.index, msg)])
            .wrap_err("Failed to send logs")?;

        let timeline_request = TimelineRequest {
            index: self.index,
            state: TimelineRequestStepState::Failed {
                outcome: TimelineRequestStepOutcome::Failed,
            },
        };

        tracing::debug!("Posting step state: {timeline_request:?}");
        self.worker_client
            .post_step_timeline(ctx.clone(), &timeline_request)
            .wrap_err("Failed to post step timeline")?;
        tracing::debug!("Posted step state: {timeline_request:?}");

        tracing::debug!("Posted setup step");
        Ok(false)
    }
}

fn read_and_send_line<I>(index: u32, tx: SyncSender<LogLine>, it: I) -> Result<()>
where
    I: Iterator<Item = Result<String, std::io::Error>>,
{
    for line in it {
        let line = line.into_diagnostic().wrap_err("Failed to read line")?;
        if let Err(e) = tx.send(LogLine::stdout(index, &line)) {
            tracing::error!("Failed to send stdout to channel, stopping the stream: {e:?}");
            bail!("stdout send failed");
        };
    }
    Ok(())
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
    w: &mut ZipWriter<File>,
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

    let mut f = File::open(src)
        .into_diagnostic()
        .wrap_err("Failed to open file to zip")?;

    std::io::copy(&mut f, w)
        .into_diagnostic()
        .wrap_err("Failed to copy content from file to zip")?;

    Ok(())
}

#[tracing::instrument(skip(w, option))]
fn add_dir_to_zip<T>(
    w: &mut ZipWriter<File>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use client::job::{LogDestination, MockJobClient};
    use jobengine::{ProjectMeta, WorkflowMeta, WorkflowRevisionMeta};
    use std::{
        collections::{BTreeMap, BTreeSet},
        env,
        sync::Arc,
    };
    use zip::ZipArchive;

    struct TestDir {
        dir: String,
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            if let Err(e) = fs::remove_dir_all(&self.dir) {
                eprintln!("Failed to remove test directory: {e:?}");
            }
        }
    }

    fn new_jobengine_context(name: &str) -> jobengine::Config {
        jobengine::Config {
            id: Uuid::now_v7(),
            name: name.to_string(),
            scans: {
                let mut m = BTreeMap::new();
                m.insert(name.to_string(), vec![]);
                m
            },
            project: ProjectMeta { id: Uuid::now_v7() },
            workflow: WorkflowMeta { id: Uuid::now_v7() },
            revision: WorkflowRevisionMeta { id: Uuid::now_v7() },
            vars: BTreeMap::new(),
            envs: BTreeMap::new(),
            inputs: None,
        }
    }

    fn new_test_workdir() -> TestDir {
        let dir = env::temp_dir()
            .join(Uuid::new_v4().to_string())
            .to_string_lossy()
            .to_string();
        fs::create_dir_all(&dir).expect("create dir all should be ok");
        TestDir { dir }
    }

    #[test]
    fn test_setup_step() {
        let mut job_client = MockJobClient::new();
        job_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 0, "{:?}", timeline);
                assert!(
                    matches!(timeline.state, TimelineRequestStepState::Running),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        job_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 0, "{:?}", timeline);
                assert!(
                    matches!(timeline.state, TimelineRequestStepState::Succeeded),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        job_client
            .expect_send_job_logs()
            .returning(|_, log| {
                assert_eq!(log.len(), 1);
                assert!(matches!(
                    log[0],
                    LogLine {
                        dst: LogDestination::Stdout,
                        step_index: 0,
                        ..
                    }
                ));
                Ok(())
            })
            .once();

        let config = new_jobengine_context("example");

        let test_dir = new_test_workdir();
        let context = ExecutionContext::new(test_dir.dir.clone(), Arc::new(vec![]), config);

        let setup_step = SetupStep {
            index: 0,
            context: &context,
            worker_client: job_client,
        };

        let result = setup_step
            .run(ctx::background())
            .expect("want setup step run to be ok, got error");

        assert!(result);

        let path = context.job_dir();
        assert!(path.exists());
    }

    #[test]
    fn test_teardown_step() {
        let mut job_client = MockJobClient::new();
        job_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 3, "{:?}", timeline);
                assert!(
                    matches!(timeline.state, TimelineRequestStepState::Running),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        job_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 3, "{:?}", timeline);
                assert!(
                    matches!(timeline.state, TimelineRequestStepState::Succeeded),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        job_client
            .expect_send_job_logs()
            .returning(|_, log| {
                assert_eq!(log.len(), 1);
                assert!(matches!(
                    log[0],
                    LogLine {
                        dst: LogDestination::Stdout,
                        step_index: 3,
                        ..
                    }
                ));
                Ok(())
            })
            .once();

        let config = new_jobengine_context("example");

        let test_dir = new_test_workdir();
        let context = ExecutionContext::new(test_dir.dir.clone(), Arc::new(vec![]), config);

        let teardown_step = TeardownStep {
            index: 3,
            context: &context,
            worker_client: job_client,
        };

        fs::create_dir_all(context.job_dir()).expect("job workdir to be created");

        let result = teardown_step
            .run(ctx::background())
            .expect("want setup step run to be ok, got error");

        assert!(result);

        let path = context.job_dir();
        assert!(!path.exists());
    }

    #[test]
    fn test_command_step_split_shell_empty() {
        let mut job_client = MockJobClient::new();

        job_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 1, "{:?}", timeline);
                assert!(
                    matches!(
                        timeline.state,
                        TimelineRequestStepState::Failed {
                            outcome: TimelineRequestStepOutcome::Failed
                        }
                    ),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        job_client
            .expect_send_job_logs()
            .returning(|_, log| {
                assert_eq!(log.len(), 1);
                assert!(matches!(
                    log[0],
                    LogLine {
                        dst: LogDestination::Stderr,
                        step_index: 1,
                        ..
                    }
                ));
                Ok(())
            })
            .once();

        let config = new_jobengine_context("example");

        let test_dir = new_test_workdir();
        let context = ExecutionContext::new(test_dir.dir.clone(), Arc::new(vec![]), config);

        let command_step = CommandStep {
            index: 1,
            context: &context,
            worker_client: job_client,
            cond: "ok",
            run: "echo 'test'",
            shell: "",
            allow_failed: true, // outcome should still be failed since this is a precondition
        };

        let v = command_step.split_shell(ctx::background());
        assert!(v.is_err(), "expected error, got {v:?}");
    }

    #[test]
    fn test_command_step_split_shell_ok() {
        let job_client = MockJobClient::new();

        let config = new_jobengine_context("example");

        let test_dir = new_test_workdir();
        let context = ExecutionContext::new(test_dir.dir.clone(), Arc::new(vec![]), config);

        let command_step = CommandStep {
            index: 1,
            context: &context,
            worker_client: job_client,
            cond: "ok",
            run: "echo 'test'",
            shell: "bash -x",
            allow_failed: true, // outcome should still be failed since this is a precondition
        };

        let v = command_step
            .split_shell(ctx::background())
            .expect("want ok");

        assert_eq!(v.len(), 2);
        assert_eq!(v[0], "bash");
        assert_eq!(v[1], "-x");
    }

    #[test]
    fn test_command_step_write_script_ok() {
        let job_client = MockJobClient::new();

        let config = new_jobengine_context("example");

        let test_dir = new_test_workdir();
        let context = ExecutionContext::new(test_dir.dir.clone(), Arc::new(vec![]), config);

        fs::create_dir_all(context.job_dir()).expect("job dir to be set");

        let command_step = CommandStep {
            index: 1,
            context: &context,
            worker_client: job_client,
            cond: "ok",
            run: "echo 'test'",
            shell: "bash -x",
            allow_failed: true, // outcome should still be failed since this is a precondition
        };

        let result = command_step
            .write_scipt(ctx::background())
            .expect("want ok");

        assert!(result.exists());
        let script = fs::read_to_string(result).expect("read should be ok");
        assert_eq!(command_step.run, script);
    }

    #[test]
    fn test_command_step_write_script_fail() {
        let mut job_client = MockJobClient::new();

        job_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 1, "{:?}", timeline);
                assert!(
                    matches!(
                        timeline.state,
                        TimelineRequestStepState::Failed {
                            outcome: TimelineRequestStepOutcome::Failed
                        }
                    ),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        job_client
            .expect_send_job_logs()
            .returning(|_, log| {
                assert_eq!(log.len(), 1);
                assert!(matches!(
                    log[0],
                    LogLine {
                        dst: LogDestination::Stderr,
                        step_index: 1,
                        ..
                    }
                ));
                Ok(())
            })
            .once();

        let config = new_jobengine_context("example");

        let test_dir = new_test_workdir();
        let context = ExecutionContext::new(test_dir.dir.clone(), Arc::new(vec![]), config);

        // directory doesn't exist. Want to make write to fail

        let command_step = CommandStep {
            index: 1,
            context: &context,
            worker_client: job_client,
            cond: "ok",
            run: "echo 'test'",
            shell: "bash -x",
            allow_failed: true, // outcome should still be failed since this is a precondition
        };

        let result = command_step.write_scipt(ctx::background());
        assert!(result.is_err(), "Expectet error, got ok: {result:?}");
    }

    #[test]
    fn test_command_step_fail_state() {
        // I know this is an overkill and other way of writing fail_state, but if
        // something shitty happens, want to have this covered, and the test is quite
        // easy to write
        let config = new_jobengine_context("example");

        let test_dir = new_test_workdir();
        let context = ExecutionContext::new(test_dir.dir.clone(), Arc::new(vec![]), config);
        let command_step = CommandStep {
            index: 1,
            context: &context,
            worker_client: MockJobClient::new(),
            cond: "ok",
            run: "echo 'test'",
            shell: "bash -x",
            allow_failed: true, // outcome should still be failed since this is a precondition
        };

        assert!(matches!(
            command_step.soft_fail_state(),
            TimelineRequestStepState::Failed {
                outcome: TimelineRequestStepOutcome::Succeeded
            }
        ));

        let command_step = CommandStep {
            index: 1,
            context: &context,
            worker_client: MockJobClient::new(),
            cond: "ok",
            run: "echo 'test'",
            shell: "bash -x",
            allow_failed: false, // outcome should still be failed since this is a precondition
        };

        assert!(matches!(
            command_step.soft_fail_state(),
            TimelineRequestStepState::Failed {
                outcome: TimelineRequestStepOutcome::Failed
            }
        ));
    }

    #[test]
    fn test_command_step_should_run_skipped() {
        let mut job_client = MockJobClient::new();

        job_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 1, "{:?}", timeline);
                assert!(
                    matches!(timeline.state, TimelineRequestStepState::Skipped),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        let config = new_jobengine_context("example");

        let test_dir = new_test_workdir();
        let mut context = ExecutionContext::new(test_dir.dir.clone(), Arc::new(vec![]), config);
        context.set_ok(false);

        let command_step = CommandStep {
            index: 1,
            context: &context,
            worker_client: job_client,
            cond: "ok",
            run: "echo 'test'",
            shell: "bash -x",
            allow_failed: false, // outcome should still be failed since this is a precondition
        };

        let result = command_step
            .should_run(ctx::background())
            .expect("to be ok");
        assert!(!result);
    }

    #[test]
    fn test_command_step_should_run_running() {
        let mut job_client = MockJobClient::new();

        job_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 1, "{:?}", timeline);
                assert!(
                    matches!(timeline.state, TimelineRequestStepState::Running),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        let config = new_jobengine_context("example");

        let test_dir = new_test_workdir();
        let context = ExecutionContext::new(test_dir.dir.clone(), Arc::new(vec![]), config);

        let command_step = CommandStep {
            index: 1,
            context: &context,
            worker_client: job_client,
            cond: "ok",
            run: "echo 'test'",
            shell: "bash -x",
            allow_failed: false, // outcome should still be failed since this is a precondition
        };

        let result = command_step
            .should_run(ctx::background())
            .expect("to be ok");
        assert!(result);
    }

    #[test]
    fn test_command_step_should_run_eval_error() {
        let mut job_client = MockJobClient::new();

        job_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 1, "{:?}", timeline);
                assert!(
                    matches!(
                        timeline.state,
                        TimelineRequestStepState::Failed {
                            outcome: TimelineRequestStepOutcome::Failed
                        }
                    ),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        job_client
            .expect_send_job_logs()
            .returning(|_, log| {
                assert_eq!(log.len(), 1);
                assert!(matches!(
                    log[0],
                    LogLine {
                        dst: LogDestination::Stderr,
                        step_index: 1,
                        ..
                    }
                ));
                Ok(())
            })
            .once();

        let config = new_jobengine_context("example");

        let test_dir = new_test_workdir();
        let context = ExecutionContext::new(test_dir.dir.clone(), Arc::new(vec![]), config);

        let command_step = CommandStep {
            index: 1,
            context: &context,
            worker_client: job_client,
            cond: "notexist",
            run: "echo 'test'",
            shell: "bash -x",
            allow_failed: false, // outcome should still be failed since this is a precondition
        };

        let result = command_step.should_run(ctx::background());

        assert!(result.is_err(), "Expected error, got {result:?}");
    }

    #[test]
    fn test_normalize_abs_path_cannot_escape() {
        let test_dir = new_test_workdir();
        let path = PathBuf::from("../../../../../etc/hosts");
        let result = normalize_abs_path(&PathBuf::from(test_dir.dir.as_str()), &path);
        assert!(result.is_err(), "Expected error, got {result:?}");
    }

    fn visit_dirs(
        base: &Path,
        dir: &Path,
        files: &mut BTreeMap<String, String>,
        dirs: &mut BTreeSet<String>,
    ) -> std::io::Result<()> {
        if dir.is_dir() {
            for entry in fs::read_dir(dir)? {
                let entry = entry?;
                let path = entry.path();
                let path_str = path
                    .strip_prefix(base)
                    .expect("strip prefix to succeed")
                    .to_string_lossy()
                    .to_string();

                if path.is_dir() {
                    dirs.insert(path_str);
                    visit_dirs(base, &path, files, dirs)?;
                } else {
                    let s = fs::read_to_string(path).expect("Failed to read string from path");
                    files.insert(path_str, s);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn test_upload_step_create_zip_file() {
        let test_dir = new_test_workdir();

        let config = new_jobengine_context("example");
        let context = ExecutionContext::new(test_dir.dir.clone(), Arc::new(vec![]), config);
        let uploads = vec![
            "file_0.txt".to_string(),
            "folder/".to_string(),
            "example/file_3.txt".to_string(),
        ];
        let job_dir = context.job_dir();
        fs::create_dir_all(job_dir).expect("create job dir");

        let (file_0, file_0_path) = ("0", "file_0.txt");
        fs::write(job_dir.join(file_0_path), file_0).expect("to write file_0");

        let zip_entire_folder_path = context.job_dir().join(&uploads[1]);
        fs::create_dir_all(&zip_entire_folder_path).expect("to create test folder in job dir");
        let (file_1, file_1_path) = ("1", "folder/file_1.txt");
        fs::write(job_dir.join(file_1_path), file_1).expect("to write file_1");
        let (file_2, file_2_path) = ("2", "folder/file_2.txt");
        fs::write(job_dir.join(file_2_path), file_2).expect("to write file_2");

        let zip_in_folder_path = job_dir.join("example/");
        fs::create_dir_all(&zip_in_folder_path).expect("to create zip in folder path");
        let (file_3, file_3_path) = ("3", "example/file_3.txt");
        fs::write(job_dir.join(file_3_path), file_3).expect("to write file_3");

        fs::write(
            job_dir.join("example/ignored.txt"),
            "this should be ignored",
        )
        .expect("to write file that will be ignored");

        let upload_step = UploadStep {
            index: 2,
            context: &context,
            worker_client: MockJobClient::new(),
            uploads: &uploads,
        };

        let zip_path = upload_step
            .create_zip_file()
            .expect("create zip file to succeed");

        assert!(zip_path.is_file());

        let result_dir = new_test_workdir();
        {
            let f = File::open(&zip_path).expect("to be able to open the result zip path");
            let mut archive = ZipArchive::new(f).expect("to create zip archive from file");
            archive
                .extract(&result_dir.dir)
                .expect("to extract the zip file to the result dir");
        }

        let mut want_files = BTreeMap::new();
        want_files.insert(file_0_path.to_string(), file_0.to_string());
        want_files.insert(file_1_path.to_string(), file_1.to_string());
        want_files.insert(file_2_path.to_string(), file_2.to_string());
        want_files.insert(file_3_path.to_string(), file_3.to_string());

        let mut want_dirs = BTreeSet::new();
        want_dirs.insert("folder".to_string());
        want_dirs.insert("example".to_string());

        let mut got_files = BTreeMap::new();
        let mut got_dirs = BTreeSet::new();

        visit_dirs(
            &PathBuf::from(&result_dir.dir),
            &PathBuf::from(&result_dir.dir),
            &mut got_files,
            &mut got_dirs,
        )
        .expect("to read in got files and got dirs");

        assert_eq!(want_files, got_files);
        assert_eq!(want_dirs, got_dirs);
    }
}

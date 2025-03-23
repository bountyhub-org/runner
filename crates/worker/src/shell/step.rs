use super::execution_context::ExecutionContext;
use cellang::Value;
use client::job::{JobClient, TimelineRequest, TimelineRequestStepOutcome};
use client::job::{LogLine, TimelineRequestStepState};
use ctx::{Background, Ctx};
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, TryRecvError};
use std::thread::JoinHandle;
use std::time::Duration;
use std::{fs, thread};
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

        tracing::debug!("Creating job workdir '{workdir:?}'");
        match fs::create_dir_all(workdir) {
            Ok(_) => {
                let msg = format!("Sucessfully created job workdir '{workdir:?}'");
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
                let msg = format!("Failed to create job workdir '{workdir:?}': {e:?}");
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

        tracing::debug!("Creating job workdir '{workdir:?}'");
        match fs::remove_dir_all(workdir) {
            Ok(_) => {
                let msg = format!("Sucessfully removed job workdir '{workdir:?}'");
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
                let msg = format!("Failed to remove job workdir '{workdir:?}': {e:?}");
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
                    state: self.fail_state(),
                };

                tracing::debug!("Posting step state: {timeline_request:?}");
                self.worker_client
                    .post_step_timeline(ctx.clone(), &timeline_request)
                    .wrap_err("Failed to post step timeline")?;

                return Ok(self.allow_failed);
            }
        };

        let stdout = child.stdout.take().expect("Failed to open stdout");
        let stderr = child.stderr.take().expect("Failed to open stderr");

        let (log_tx, log_rx) = mpsc::sync_channel(100);

        let stdout_tx = log_tx.clone();
        let index = self.index;
        let stdout_handle = thread::spawn(move || {
            let lines = BufReader::new(stdout).lines();
            for line in lines {
                let line = line.unwrap();
                if let Err(e) = stdout_tx.send(LogLine::stdout(index, &line)) {
                    tracing::error!("Failed to send stdout to channel, stopping the stream: {e:?}");
                    bail!("stdout send failed");
                };
            }
            Ok(())
        });

        let stderr_tx = log_tx.clone();
        let index = self.index;
        let stderr_handle = thread::spawn(move || {
            let lines = BufReader::new(stderr).lines();
            for line in lines {
                let line = line.unwrap();
                if let Err(e) = stderr_tx.send(LogLine::stderr(index, &line)) {
                    tracing::error!("Failed to send stderr to channel, stopping the stream: {e:?}");
                    bail!("stderr send failed");
                };
            }
            Ok(())
        });

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
                        state: self.fail_state(),
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
                        self.fail_state()
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
                    state: self.fail_state(),
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
        match self
            .context
            .eval_expr(self.cond)
            .wrap_err("Condition evaluation failed")?
        {
            Value::Bool(false) => {
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
            Value::Bool(true) => {
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
            v => {
                let msg = format!("Failed to evaluate the if condition: '{}'", self.cond);
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

    fn fail_state(&self) -> TimelineRequestStepState {
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
                state: TimelineRequestStepState::Running,
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

        let workdir = PathBuf::from(self.context.workdir());

        let filename = format!("{}.zip", Uuid::new_v4());
        let result_path = workdir.join(filename);

        let uploads = self.uploads.clone();
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

#[cfg(test)]
mod tests {
    use super::*;
    use client::job::{LogDestination, MockJobClient};
    use jobengine::{ProjectMeta, WorkflowMeta, WorkflowRevisionMeta};
    use std::{collections::BTreeMap, env, sync::Arc};

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
        TestDir {
            dir: env::temp_dir()
                .join(Uuid::new_v4().to_string())
                .to_string_lossy()
                .to_string(),
        }
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
            command_step.fail_state(),
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
            command_step.fail_state(),
            TimelineRequestStepState::Failed {
                outcome: TimelineRequestStepOutcome::Failed
            }
        ));
    }
}

use super::execution_context::ExecutionContext;
use artifact::ArtifactBuilder;
use cellang::Value;
use client::job::{
    JobClient, LogDestination, LogLine, Step, TimelineRequest, TimelineRequestStepOutcome,
    TimelineRequestStepState,
};
use ctx::{Background, Ctx};
use error_stack::{Context, Report, Result, ResultExt};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, TryRecvError};
use std::time::Duration;
use std::{fmt, thread};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug)]
pub(crate) struct StepsRunner {
    steps: Vec<Step>,
    execution_ctx: ExecutionContext,
}

impl StepsRunner {
    pub(crate) fn new(execution_ctx: ExecutionContext, steps: Vec<Step>) -> Self {
        Self {
            steps,
            execution_ctx,
        }
    }

    #[tracing::instrument(skip(self, ctx, client))]
    pub(crate) fn run<C>(&mut self, ctx: Ctx<Background>, client: &C) -> Result<(), ExecutionError>
    where
        C: JobClient,
    {
        let stream_handle;
        {
            let (tx, rx) = mpsc::channel();
            let log_client = client.clone();
            let log_ctx = ctx.clone();
            stream_handle = thread::spawn(move || {
                let mut buffer = Vec::with_capacity(64);

                'outer: loop {
                    buffer.clear();
                    thread::sleep(std::time::Duration::from_secs(1));

                    loop {
                        match rx.try_recv() {
                            Ok(line) => buffer.push(line),
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => break 'outer,
                        }
                    }

                    if buffer.is_empty() {
                        continue;
                    }

                    if let Err(e) = log_client.send_job_logs(log_ctx.clone(), buffer.clone()) {
                        tracing::error!("sending job logs failed: {e:?}");
                    }
                }

                if buffer.is_empty() {
                    return;
                }

                if let Err(e) = log_client.send_job_logs(log_ctx, buffer.clone()) {
                    tracing::error!("error sending job log: {e:?}");
                }
            });

            for (index, step) in self.steps.iter().enumerate() {
                let tx = tx.clone();
                let ctx = &ctx;
                tracing::debug!("Step: {:?}", step);
                let index = index as u32;
                match self.should_run_step(step) {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(e) => {
                        // If evaluation failed, which should not occur, fail the run
                        tracing::debug!("Posting step {step:?} as failed");
                        client
                            .post_step_timeline(
                                ctx.clone(),
                                &TimelineRequest {
                                    index,
                                    state: TimelineRequestStepState::Failed {
                                        outcome: TimelineRequestStepOutcome::Failed,
                                    },
                                },
                            )
                            .change_context(ExecutionError)
                            .attach_printable(format!(
                                "failed to post a timeline for step {index}"
                            ))?;

                        return Err(e);
                    }
                };

                tracing::debug!("Posting step {step:?} as running");
                client
                    .post_step_timeline(
                        ctx.clone(),
                        &TimelineRequest {
                            index,
                            state: TimelineRequestStepState::Running,
                        },
                    )
                    .change_context(ExecutionError)
                    .attach_printable(format!("Failed to post step {index} as running "))?;

                tracing::debug!("Starting the log stream for step {step:?}");

                tracing::debug!("Starting step execution");
                let state = match step {
                    Step::Setup => self.run_setup(ctx.clone(), index, tx),
                    Step::Teardown => self.run_teardown(ctx.clone(), index, tx),
                    Step::Upload { uploads } => {
                        let step = UploadStep { uploads, client };
                        step.run(ctx.clone(), &mut self.execution_ctx, index, tx)
                    }
                    Step::Command {
                        run,
                        shell,
                        allow_failed,
                        ..
                    } => {
                        let cmd = CommandStep {
                            run,
                            shell,
                            allow_failed: *allow_failed,
                        };

                        cmd.run(ctx.clone(), &mut self.execution_ctx, index, tx)
                    }
                };

                client
                    .post_step_timeline(ctx.clone(), &TimelineRequest { index, state })
                    .change_context(ExecutionError)
                    .attach_printable(format!("Failed to post step's timeline for step {index}"))?;

                self.execution_ctx.update_state(state);
            }
        };

        tracing::info!("waiting for stream handle to be joined");

        stream_handle
            .join()
            .expect("Stream handle should join successfully");

        Ok(())
    }

    #[tracing::instrument(skip(self))]
    fn should_run_step(&self, step: &Step) -> Result<bool, ExecutionError> {
        tracing::debug!("Evaluating condition for step: {step:?}");
        match step {
            Step::Command { cond, .. } => match self.execution_ctx.eval_expr(cond) {
                Ok(val) => match val {
                    Value::Bool(v) => Ok(v),
                    v => Err(Report::new(ExecutionError).attach_printable(format!(
                        "Expected boolean expression in condition '{cond}', got {v:?}"
                    ))),
                },
                Err(e) => Err(Report::new(ExecutionError)
                    .attach_printable(format!("Condition evaluation failed: {e}"))),
            },
            Step::Upload { .. } => Ok(self.execution_ctx.ok()),
            _ => Ok(true),
        }
    }

    #[tracing::instrument(skip(self, _ctx, log_tx))]
    fn run_setup(
        &self,
        _ctx: Ctx<Background>,
        step_index: u32,
        log_tx: mpsc::Sender<LogLine>,
    ) -> TimelineRequestStepState {
        let workdir = self.execution_ctx.workdir();
        tracing::debug!("Running create_dir_all for workdir '{workdir}'");

        match fs::create_dir_all(workdir) {
            Ok(_) => {
                log_tx
                    .send(LogLine {
                        dst: LogDestination::Stdout,
                        step_index,
                        timestamp: OffsetDateTime::now_utc(),
                        line: format!("Created workdir: '{}'", workdir),
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));
                TimelineRequestStepState::Succeeded
            }
            Err(e) => {
                log_tx
                    .send(LogLine {
                        dst: LogDestination::Stderr,
                        step_index,
                        timestamp: OffsetDateTime::now_utc(),
                        line: format!("Failed to create workdir '{}': {}", workdir, e),
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));

                TimelineRequestStepState::Failed {
                    outcome: TimelineRequestStepOutcome::Failed,
                }
            }
        }
    }

    #[tracing::instrument(skip(self, _ctx, log_tx))]
    fn run_teardown(
        &self,
        _ctx: Ctx<Background>,
        step_index: u32,
        log_tx: mpsc::Sender<LogLine>,
    ) -> TimelineRequestStepState {
        let workdir = self.execution_ctx.workdir();
        tracing::info!("Removing workdir: {}", workdir);
        match fs::remove_dir_all(workdir) {
            Ok(_) => {
                log_tx
                    .send(LogLine {
                        dst: LogDestination::Stdout,
                        step_index,
                        timestamp: OffsetDateTime::now_utc(),
                        line: format!("Removed workdir: {}", workdir),
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));
                TimelineRequestStepState::Succeeded
            }
            Err(e) => {
                log_tx
                    .send(LogLine {
                        dst: LogDestination::Stderr,
                        step_index,
                        timestamp: OffsetDateTime::now_utc(),
                        line: format!("Failed to remove workdir '{}': {}", workdir, e),
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));
                TimelineRequestStepState::Failed {
                    outcome: TimelineRequestStepOutcome::Succeeded,
                }
            }
        }
    }
}

#[derive(Debug)]
struct CommandStep<'a> {
    run: &'a str,
    shell: &'a str,
    allow_failed: bool,
}

impl CommandStep<'_> {
    #[tracing::instrument(skip(ctx, execution_ctx, log_tx))]
    fn run(
        &self,
        ctx: Ctx<Background>,
        execution_ctx: &mut ExecutionContext,
        step_index: u32,
        log_tx: mpsc::Sender<LogLine>,
    ) -> TimelineRequestStepState {
        let mut shell_split = match shlex::split(self.shell) {
            Some(split) => split,
            None => {
                log_tx
                    .send(LogLine {
                        dst: LogDestination::Stderr,
                        step_index,
                        timestamp: OffsetDateTime::now_utc(),
                        line: format!("Spliting shell '{}' using shlex failed", self.shell),
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));

                return self.fail();
            }
        };

        let script_path = match self.write_script(ctx.clone(), execution_ctx) {
            Ok(path) => path,
            Err(err) => {
                log_tx
                    .send(LogLine {
                        dst: LogDestination::Stderr,
                        step_index,
                        timestamp: OffsetDateTime::now_utc(),
                        line: format!("Failed to write script: {}", err),
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));

                return self.fail();
            }
        };

        shell_split.push(
            script_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string(),
        );

        let command = shell_split.remove(0);
        let mut cmd = Command::new(command);
        cmd.args(shell_split);
        cmd.current_dir(execution_ctx.workdir());
        cmd.envs(execution_ctx.envs().iter().cloned());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        tracing::info!("Spawning command: {cmd:?}");
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                tracing::error!("Failed to spawn child process: {err:?}");
                return self.fail();
            }
        };

        let stdout = child.stdout.take().expect("Failed to open stdout");
        let stderr = child.stderr.take().expect("Failed to open stderr");

        let stdout_tx = log_tx.clone();
        let stdout_handle = thread::spawn(move || {
            let lines = BufReader::new(stdout).lines();
            for line in lines {
                let line = line.unwrap();
                stdout_tx
                    .send(LogLine {
                        dst: LogDestination::Stdout,
                        step_index,
                        timestamp: OffsetDateTime::now_utc(),
                        line,
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));
            }
        });

        let stderr_tx = log_tx.clone();
        let stderr_handle = thread::spawn(move || {
            let lines = BufReader::new(stderr).lines();
            for line in lines {
                let line = line.unwrap();
                stderr_tx
                    .send(LogLine {
                        dst: LogDestination::Stderr,
                        step_index,
                        timestamp: OffsetDateTime::now_utc(),
                        line,
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));
            }
        });

        tracing::info!("Waiting for command to finish");
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if !ctx.is_done() {
                        thread::sleep(Duration::from_millis(250));
                        continue;
                    }
                    log_tx
                        .send(LogLine {
                            dst: LogDestination::Stderr,
                            step_index,
                            timestamp: OffsetDateTime::now_utc(),
                            line: "Received cancellation signal. Killing child process".to_string(),
                        })
                        .unwrap_or_else(|err| {
                            tracing::error!("Failed to send log line: {:?}", err)
                        });

                    tracing::info!("Received cancellation signal. Killing child process");
                    if let Err(err) = child.kill() {
                        tracing::error!("Failed to kill child process: {}. Command might still execute in the background", err);
                    } else {
                        tracing::info!("Killed child process");
                    }

                    return TimelineRequestStepState::Cancelled;
                }
                Err(err) => {
                    tracing::error!("Failed to wait for child process: {:?}", err);
                    log_tx
                        .send(LogLine {
                            dst: LogDestination::Stderr,
                            step_index,
                            timestamp: OffsetDateTime::now_utc(),
                            line: format!("Failed to wait for child process: {}", err),
                        })
                        .unwrap_or_else(|err| {
                            tracing::error!("Failed to send log line: {:?}", err)
                        });
                    return self.fail();
                }
            }
        }
        let exit_code = match child.wait() {
            Ok(out) => out.code().unwrap_or(1),
            Err(err) => {
                tracing::error!("Failed to wait for child process: {:?}", err);
                1
            }
        };

        stdout_handle.join().expect("Failed to join stdout handle");
        stderr_handle.join().expect("Failed to join stderr handle");

        if exit_code != 0 {
            self.fail()
        } else {
            TimelineRequestStepState::Succeeded
        }
    }

    fn fail(&self) -> TimelineRequestStepState {
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

    fn write_script(
        &self,
        ctx: Ctx<Background>,
        execution_ctx: &ExecutionContext,
    ) -> Result<PathBuf, ExecutionError> {
        let code = execution_ctx
            .eval_templ(self.run)
            .change_context(ExecutionError)
            .attach_printable("failed to parse code")?;

        if ctx.is_done() {
            return Err(Report::new(ContextCancelledError).change_context(ExecutionError));
        }

        let file_path = Path::new(execution_ctx.workdir()).join(Uuid::new_v4().to_string());
        {
            let mut f = match File::create(&file_path) {
                Ok(f) => f,
                Err(err) => {
                    return Err(Report::new(ExecutionError)
                        .attach_printable(format!("Failed to crete file {file_path:?}: {err:?}")));
                }
            };

            if let Err(err) = f.write_all(code.as_bytes()) {
                return Err(Report::new(ExecutionError)
                    .attach_printable(format!("Failed to write bytes into a file: {err:?}")));
            }
        }

        Ok(file_path)
    }
}

struct UploadStep<'a, C>
where
    C: JobClient,
{
    uploads: &'a Vec<String>,
    client: &'a C,
}

impl<C> UploadStep<'_, C>
where
    C: JobClient,
{
    #[tracing::instrument(skip(self, ctx, execution_ctx))]
    fn run(
        &self,
        ctx: Ctx<Background>,
        execution_ctx: &mut ExecutionContext,
        step_index: u32,
        log_tx: mpsc::Sender<LogLine>,
    ) -> TimelineRequestStepState {
        let artifact_builder = match ArtifactBuilder::new(
            &PathBuf::from(&execution_ctx.workdir()),
            self.uploads.clone(),
        ) {
            Ok(ab) => ab,
            Err(e) => {
                log_tx
                    .send(LogLine {
                        dst: LogDestination::Stderr,
                        step_index,
                        timestamp: OffsetDateTime::now_utc(),
                        line: format!("Failed to upload job result: {e:?}"),
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));
                return TimelineRequestStepState::Failed {
                    outcome: TimelineRequestStepOutcome::Failed,
                };
            }
        };

        tracing::info!("Building an artifact");
        let artifact = match artifact_builder.build(ctx::background()) {
            Ok(artifact) => artifact,
            Err(err) => {
                tracing::error!("Failed to build the artifact: {:?}", err);
                return TimelineRequestStepState::Failed {
                    outcome: TimelineRequestStepOutcome::Failed,
                };
            }
        };

        tracing::info!("Opening artifact file: {:?}", artifact.file);
        let file = match File::open(&artifact.file) {
            Ok(file) => file,
            Err(err) => {
                tracing::error!("Failed to open artifact file: {:?}", err);
                return TimelineRequestStepState::Failed {
                    outcome: TimelineRequestStepOutcome::Failed,
                };
            }
        };

        tracing::info!("Uploading job result");
        match self.client.upload_job_artifact(ctx.clone(), file) {
            Ok(_) => {
                tracing::info!("Uploaded job result");
                log_tx
                    .send(LogLine {
                        dst: LogDestination::Stdout,
                        step_index,
                        timestamp: OffsetDateTime::now_utc(),
                        line: "Uploaded job result".to_string(),
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));
                TimelineRequestStepState::Succeeded
            }
            Err(err) => {
                tracing::error!("Failed to upload job result: {:?}", err);
                log_tx
                    .send(LogLine {
                        dst: LogDestination::Stderr,
                        step_index,
                        timestamp: OffsetDateTime::now_utc(),
                        line: format!("Failed to upload job result: {}", err),
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));
                TimelineRequestStepState::Failed {
                    outcome: TimelineRequestStepOutcome::Failed,
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct StepError;

impl fmt::Display for StepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Step error")
    }
}

impl Context for StepError {}

#[derive(Debug)]
pub struct ExecutionError;

impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Execution error")
    }
}

impl Context for ExecutionError {}

#[derive(Debug)]
pub struct ContextCancelledError;

impl fmt::Display for ContextCancelledError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Context cancelled")
    }
}

impl Context for ContextCancelledError {}

#[cfg(test)]
mod tests {
    use super::*;
    use client::job::{MockJobClient, TimelineRequestStepState};
    use jobengine::{ProjectMeta, WorkflowMeta, WorkflowRevisionMeta};
    use std::fs;
    use std::sync::Arc;
    use std::{collections::BTreeMap, env};
    use uuid::Uuid;

    fn new_test_job_config() -> jobengine::Config {
        jobengine::Config {
            id: Uuid::now_v7(),
            name: "example".to_string(),
            scans: BTreeMap::new(),
            project: ProjectMeta { id: Uuid::now_v7() },
            workflow: WorkflowMeta { id: Uuid::now_v7() },
            revision: WorkflowRevisionMeta { id: Uuid::now_v7() },
            vars: BTreeMap::new(),
            envs: BTreeMap::new(),
            inputs: None,
        }
    }

    fn new_test_workdir() -> String {
        env::temp_dir()
            .join(Uuid::new_v4().to_string())
            .to_string_lossy()
            .to_string()
    }

    #[test]
    fn test_step_timeline_posting() {
        let mut main_client = MockJobClient::new();

        // Setup step running
        main_client
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

        // Setup step succeeded
        main_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 0, "{:?}", timeline);
                assert!(
                    matches!(timeline.state, TimelineRequestStepState::Succeeded { .. }),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        // Command step running
        main_client
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

        // Command step succeeded
        main_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 1, "{:?}", timeline);
                assert!(
                    matches!(timeline.state, TimelineRequestStepState::Succeeded { .. }),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        // Step index 2 is skipped

        // Step index 3 is running
        main_client
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

        // Step index 3 is failed with allow_failed
        main_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 3, "{:?}", timeline);
                assert!(
                    matches!(
                        timeline.state,
                        TimelineRequestStepState::Failed {
                            outcome: TimelineRequestStepOutcome::Succeeded
                        }
                    ),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        // Step index 4 is running
        main_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 4, "{:?}", timeline);
                assert!(
                    matches!(timeline.state, TimelineRequestStepState::Running),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        // Step index 4 is failed
        main_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 4, "{:?}", timeline);
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

        // Step index 5 is executed since it is teardown
        main_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 5, "{:?}", timeline);
                assert!(
                    matches!(timeline.state, TimelineRequestStepState::Running),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        // Step index 5 is failed
        main_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.index, 5, "{:?}", timeline);
                assert!(
                    matches!(timeline.state, TimelineRequestStepState::Succeeded),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        main_client
            .expect_clone()
            .returning(|| {
                let mut stream = MockJobClient::new();
                stream
                    .expect_send_job_logs()
                    .returning(|_, _| Ok(()))
                    .times(1..);
                stream
            })
            .times(1..);

        let execution_ctx =
            ExecutionContext::new(new_test_workdir(), Arc::new(vec![]), new_test_job_config());

        let mut runner = StepsRunner::new(
            execution_ctx,
            vec![
                Step::Setup,
                Step::Command {
                    cond: "true".to_string(),
                    run: "echo 'hello'".to_string(),
                    shell: "bash".to_string(),
                    allow_failed: false,
                },
                Step::Command {
                    cond: "false".to_string(),
                    run: "exit 1".to_string(),
                    shell: "sh".to_string(),
                    allow_failed: true,
                },
                Step::Command {
                    cond: "true".to_string(),
                    run: "exit 1".to_string(),
                    shell: "sh".to_string(),
                    allow_failed: true,
                },
                Step::Command {
                    cond: "true".to_string(),
                    run: "exit 1".to_string(),
                    shell: "sh".to_string(),
                    allow_failed: false,
                },
                Step::Teardown,
            ],
        );
        let result = runner.run(ctx::background(), &main_client);
        assert!(result.is_ok(), "{:?}", result);
    }

    #[test]
    fn test_command_step_hello_world() {
        let step = CommandStep {
            run: "echo 'Hello, World!'",
            shell: "sh",
            allow_failed: false,
        };

        let workdir = new_test_workdir();
        fs::create_dir_all(&workdir).expect("Failed to create workdir");
        let job_execution_ctx = new_test_job_config();
        let mut execution_ctx = ExecutionContext::new(workdir, Arc::new(vec![]), job_execution_ctx);

        let (tx, rx) = mpsc::channel();
        let result = step.run(ctx::background(), &mut execution_ctx, 1, tx);
        assert!(
            matches!(result, TimelineRequestStepState::Succeeded),
            "{:?}",
            result
        );

        let log_line = rx.try_recv().expect("Failed to receive log line");
        assert!(matches!(log_line.dst, LogDestination::Stdout));
        assert_eq!(log_line.line, "Hello, World!");
        assert_eq!(log_line.step_index, 1);
    }

    #[test]
    fn test_command_step_env_propagation() {
        let step = CommandStep {
            run: "echo $HELLO_ENV",
            shell: "sh",
            allow_failed: false,
        };

        let workdir = new_test_workdir();
        fs::create_dir_all(&workdir).expect("Failed to create workdir");
        let job_execution_ctx = new_test_job_config();
        let mut execution_ctx = ExecutionContext::new(
            workdir,
            Arc::new(vec![("HELLO_ENV".to_string(), "Hello, World!".to_string())]),
            job_execution_ctx,
        );

        let (tx, rx) = mpsc::channel();
        let result = step.run(ctx::background(), &mut execution_ctx, 1, tx);
        assert!(
            matches!(result, TimelineRequestStepState::Succeeded),
            "{:?}",
            result
        );

        let log_line = rx.try_recv().expect("Failed to receive log line");
        assert!(matches!(log_line.dst, LogDestination::Stdout));
        assert_eq!(log_line.line, "Hello, World!");
        assert_eq!(log_line.step_index, 1);
    }

    #[test]
    fn test_complex_shell() {
        let step = CommandStep {
            run: "echo 'Hello, World!'",
            shell: "bash --verbose",
            allow_failed: false,
        };

        let workdir = new_test_workdir();
        fs::create_dir_all(&workdir).expect("Failed to create workdir");
        let job_execution_ctx = new_test_job_config();
        let mut execution_ctx = ExecutionContext::new(workdir, Arc::new(vec![]), job_execution_ctx);

        let (tx, rx) = mpsc::channel();
        let result = step.run(ctx::background(), &mut execution_ctx, 1, tx);
        assert!(
            matches!(result, TimelineRequestStepState::Succeeded),
            "{:?}",
            result
        );

        let mut stdout_count = 0;
        let mut stderr_count = 0;

        for _ in 0..2 {
            let log_line = rx.recv().expect("Failed to receive log line");
            match log_line.dst {
                LogDestination::Stdout => {
                    assert_eq!(log_line.line, "Hello, World!");
                    assert_eq!(log_line.step_index, 1);
                    stdout_count += 1;
                }
                LogDestination::Stderr => {
                    assert!(log_line.line.contains("echo 'Hello, World!'"));
                    assert_eq!(log_line.step_index, 1);
                    stderr_count += 1;
                }
            }
        }
        assert_eq!(stderr_count, 1);
        assert_eq!(stdout_count, 1);
    }

    #[test]
    fn test_command_step_allow_failed() {
        let step = CommandStep {
            run: "exit 1",
            shell: "sh",
            allow_failed: true,
        };

        let workdir = new_test_workdir();
        fs::create_dir_all(&workdir).expect("Failed to create workdir");
        let job_execution_ctx = new_test_job_config();
        let mut execution_ctx = ExecutionContext::new(
            workdir,
            Arc::new(vec![("HELLO_ENV".to_string(), "Hello, World!".to_string())]),
            job_execution_ctx,
        );

        let (tx, _rx) = mpsc::channel();
        let result = step.run(ctx::background(), &mut execution_ctx, 0, tx);
        assert!(
            matches!(
                result,
                TimelineRequestStepState::Failed {
                    outcome: TimelineRequestStepOutcome::Succeeded
                }
            ),
            "{:?}",
            result
        );
    }

    #[test]
    fn test_command_cancel() {
        let step = CommandStep {
            run: "sleep 2",
            shell: "sh",
            allow_failed: false,
        };

        let workdir = new_test_workdir();
        fs::create_dir_all(&workdir).expect("Failed to create workdir");
        let job_config = new_test_job_config();
        let mut execution_ctx = ExecutionContext::new(workdir, Arc::new(vec![]), job_config);

        let background = ctx::background().with_cancel();
        let ctx = background.with_cancel();

        let step_ctx = ctx.to_background();
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || step.run(step_ctx, &mut execution_ctx, 1, tx));

        thread::sleep(Duration::from_millis(500));
        ctx.cancel();

        thread::sleep(Duration::from_millis(500));
        assert!(handle.is_finished());

        let result = handle.join().expect("failed to join step handle");
        assert!(matches!(result, TimelineRequestStepState::Cancelled));

        let log_line = rx.try_recv().expect("Failed to receive log line");
        assert!(matches!(log_line.dst, LogDestination::Stderr));
        assert!(!log_line.line.is_empty());
    }
}

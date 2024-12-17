use super::execution_context::ExecutionContext;
use artifact::ArtifactBuilder;
use cel_interpreter::Value as CelValue;
use client::job::{
    JobClient, JobResolvedStepResponse, StepKind, StepRef, TimelineRequest,
    TimelineRequestStepOutcome, TimelineRequestStepState,
};
use ctx::{Background, Ctx};
use error_stack::{Context, Result, ResultExt};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use std::{fmt, thread};
use streamlog::{LogDestination, LogLine};
use templ::{Template, Token};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug)]
pub(crate) struct Steps {
    steps: Vec<Step>,
}

impl Steps {
    pub(crate) fn new(steps: Vec<JobResolvedStepResponse>, uploads: Option<Vec<String>>) -> Self {
        let steps = steps
            .into_iter()
            .map(|step| Step::new(step, &uploads))
            .collect();
        Self { steps }
    }

    #[tracing::instrument(skip(self, ctx, execution_ctx, client))]
    pub(crate) fn run<C>(
        &mut self,
        ctx: Ctx<Background>,
        execution_ctx: &mut ExecutionContext,
        client: &C,
    ) -> Result<(), ExecutionError>
    where
        C: JobClient,
    {
        for step in self.steps.iter() {
            tracing::debug!("Step: {:?}", step);
            step.run(ctx.clone(), execution_ctx, client)
                .change_context(ExecutionError)
                .attach_printable("step failed with unrecoverable error")?;
        }

        Ok(())
    }
}

#[derive(Debug)]
struct Step {
    id: Uuid,
    cond: String,
    inner: InnerStep,
}

impl Step {
    fn new(step: JobResolvedStepResponse, uploads: &Option<Vec<String>>) -> Self {
        let cond = step.cond;
        let inner = match step.kind {
            StepKind::Setup => InnerStep::Setup,
            StepKind::Teardown => InnerStep::Teardown,
            StepKind::Upload => InnerStep::Upload(UploadStep {
                uploads: uploads.clone().unwrap_or_default(),
            }),
            StepKind::Command => InnerStep::Command(CommandStep {
                run: step.run,
                shell: step.shell,
            }),
        };

        Self {
            id: step.id,
            cond,
            inner,
        }
    }

    #[tracing::instrument(skip(ctx, execution_ctx, client))]
    pub(crate) fn run<C>(
        &self,
        ctx: Ctx<Background>,
        execution_ctx: &mut ExecutionContext,
        client: &C,
    ) -> Result<(), StepError>
    where
        C: JobClient,
    {
        let state = self.pre_run_step_state(ctx.clone(), execution_ctx);
        tracing::info!("Pre run step({}) state: {}", self.id, state);
        let req = TimelineRequest { id: self.id, state };

        tracing::debug!("Posting timeline: {:?}", req);
        client
            .post_step_timeline(ctx.clone(), &req)
            .change_context(StepError)
            .attach_printable("failed to post timeline")?;

        if state.is_done() {
            execution_ctx.update_state(state);
            return Ok(());
        }

        let step_ref = StepRef {
            step_id: self.id,
            job_id: execution_ctx.job_id(),
            project_id: execution_ctx.project_id(),
            workflow_id: execution_ctx.workflow_id(),
            revision_id: execution_ctx.revision_id(),
        };
        let streamer = streamlog::Stream {
            client: client.clone(),
            step_ref,
        };

        let stream_handle = {
            let (tx, rx) = mpsc::channel();
            let stream_ctx = ctx.clone();
            let stream_handle = thread::spawn(move || streamer.stream(stream_ctx, rx));

            tracing::info!("Executing step: {}", self.id);
            let state = self.execute(ctx.clone(), execution_ctx, client.clone(), tx);

            tracing::info!("Step({}): {}", self.id, state);
            execution_ctx.update_state(state);

            let req = TimelineRequest { id: self.id, state };

            tracing::debug!("Posting timeline: {:?}", req);
            client
                .post_step_timeline(ctx.clone(), &req)
                .change_context(StepError)
                .attach_printable("failed to post timeline")?;

            stream_handle
        };

        if let Err(err) = stream_handle.join() {
            tracing::error!("Failed to join stream handle: {:?}", err);
        }

        Ok(())
    }

    #[tracing::instrument(skip(ctx, execution_ctx, client, log_tx))]
    fn execute<C>(
        &self,
        ctx: Ctx<Background>,
        execution_ctx: &mut ExecutionContext,
        client: C,
        log_tx: mpsc::Sender<LogLine>,
    ) -> TimelineRequestStepState
    where
        C: JobClient,
    {
        tracing::info!("Running step: {}", self.id);
        match &self.inner {
            InnerStep::Setup => self.run_setup(self.id, execution_ctx, log_tx),
            InnerStep::Command(step) => step.run(ctx.clone(), self.id, execution_ctx, log_tx),
            InnerStep::Upload(step) => {
                step.run(ctx.clone(), self.id, execution_ctx, &client, log_tx)
            }
            InnerStep::Teardown => self.run_teardown(self.id, execution_ctx, log_tx),
        }
    }

    #[tracing::instrument(skip(_ctx, execution_ctx))]
    fn pre_run_step_state(
        &self,
        _ctx: Ctx<Background>,
        execution_ctx: &mut ExecutionContext,
    ) -> TimelineRequestStepState {
        tracing::info!("Evaluating step condition for step: {}", self.id);

        let should_skip = match execution_ctx.eval(&self.cond) {
            Ok(result) => result == false.into(),
            Err(e) => {
                tracing::error!("Failed to evaluate condition: {:?}", e);
                return TimelineRequestStepState::Failed {
                    outcome: TimelineRequestStepOutcome::Failed,
                };
            }
        };

        if should_skip {
            tracing::info!("Skipping step: {}", self.id);
            return TimelineRequestStepState::Skipped;
        }

        TimelineRequestStepState::Running
    }
}

#[derive(Debug)]
enum InnerStep {
    Setup,
    Command(CommandStep),
    Upload(UploadStep),
    Teardown,
}

impl Step {
    #[tracing::instrument(skip(execution_ctx, log_tx))]
    fn run_setup(
        &self,
        id: Uuid,
        execution_ctx: &mut ExecutionContext,
        log_tx: mpsc::Sender<LogLine>,
    ) -> TimelineRequestStepState {
        let workdir = execution_ctx.workdir();
        tracing::info!("Creating workdir: {}", workdir);
        match fs::create_dir_all(workdir) {
            Ok(_) => {
                log_tx
                    .send(LogLine {
                        dst: streamlog::LogDestination::Stdout,
                        timestamp: OffsetDateTime::now_utc(),
                        message: format!("Created workdir: {}", workdir),
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));
                TimelineRequestStepState::Succeeded
            }
            Err(e) => {
                log_tx
                    .send(LogLine {
                        dst: streamlog::LogDestination::Stderr,
                        timestamp: OffsetDateTime::now_utc(),
                        message: format!("Failed to create workdir '{}': {}", workdir, e),
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));
                TimelineRequestStepState::Failed {
                    outcome: TimelineRequestStepOutcome::Failed,
                }
            }
        }
    }

    #[tracing::instrument(skip(self, execution_ctx))]
    fn run_teardown(
        &self,
        id: Uuid,
        execution_ctx: &mut ExecutionContext,
        log_tx: mpsc::Sender<LogLine>,
    ) -> TimelineRequestStepState {
        let workdir = execution_ctx.workdir();
        tracing::info!("Removing workdir: {}", workdir);
        match fs::remove_dir_all(workdir) {
            Ok(_) => {
                log_tx
                    .send(LogLine {
                        dst: LogDestination::Stdout,
                        timestamp: OffsetDateTime::now_utc(),
                        message: format!("Removed workdir: {}", workdir),
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));
                TimelineRequestStepState::Succeeded
            }
            Err(e) => {
                log_tx
                    .send(LogLine {
                        dst: LogDestination::Stderr,
                        timestamp: OffsetDateTime::now_utc(),
                        message: format!("Failed to remove workdir '{}': {}", workdir, e),
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
struct CommandStep {
    run: String,
    shell: String,
}

impl CommandStep {
    #[tracing::instrument(skip(ctx, execution_ctx, log_tx))]
    fn run(
        &self,
        ctx: Ctx<Background>,
        id: Uuid,
        execution_ctx: &mut ExecutionContext,
        log_tx: mpsc::Sender<LogLine>,
    ) -> TimelineRequestStepState {
        let mut shell_split = match shlex::split(&self.shell) {
            Some(split) => {
                if split.is_empty() {
                    return TimelineRequestStepState::Failed {
                        outcome: TimelineRequestStepOutcome::Failed,
                    };
                }
                split
            }
            None => {
                return TimelineRequestStepState::Failed {
                    outcome: TimelineRequestStepOutcome::Failed,
                }
            }
        };

        let script_path = match self.write_script(ctx.clone(), execution_ctx) {
            Ok(path) => path,
            Err(err) => {
                log_tx
                    .send(LogLine {
                        dst: streamlog::LogDestination::Stderr,
                        timestamp: OffsetDateTime::now_utc(),
                        message: format!("Failed to write script: {}", err),
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));
                return TimelineRequestStepState::Failed {
                    outcome: TimelineRequestStepOutcome::Failed,
                };
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

        tracing::info!("Spawning command: {:?}", cmd);
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(err) => {
                tracing::error!("Failed to spawn child process: {}", err);
                return TimelineRequestStepState::Failed {
                    outcome: TimelineRequestStepOutcome::Failed,
                };
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
                        dst: streamlog::LogDestination::Stdout,
                        timestamp: OffsetDateTime::now_utc(),
                        message: line,
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
                        dst: streamlog::LogDestination::Stderr,
                        timestamp: OffsetDateTime::now_utc(),
                        message: line,
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
                            dst: streamlog::LogDestination::Stderr,
                            timestamp: OffsetDateTime::now_utc(),
                            message: "Received cancellation signal. Killing child process"
                                .to_string(),
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
                            dst: streamlog::LogDestination::Stderr,
                            timestamp: OffsetDateTime::now_utc(),
                            message: format!("Failed to wait for child process: {}", err),
                        })
                        .unwrap_or_else(|err| {
                            tracing::error!("Failed to send log line: {:?}", err)
                        });
                    return TimelineRequestStepState::Failed {
                        outcome: TimelineRequestStepOutcome::Failed,
                    };
                }
            }
        }
        let exit_code = match child.wait() {
            Ok(out) => out.code().unwrap_or(1),
            Err(err) => {
                tracing::error!("Failed to wait for child process: {:?}", err);
                return TimelineRequestStepState::Failed {
                    outcome: TimelineRequestStepOutcome::Failed,
                };
            }
        };

        stdout_handle.join().expect("Failed to join stdout handle");
        stderr_handle.join().expect("Failed to join stderr handle");

        if exit_code != 0 {
            return TimelineRequestStepState::Failed {
                outcome: TimelineRequestStepOutcome::Failed,
            };
        }

        TimelineRequestStepState::Succeeded
    }

    fn write_script(
        &self,
        ctx: Ctx<Background>,
        execution_ctx: &ExecutionContext,
    ) -> std::result::Result<PathBuf, String> {
        let Template { tokens } = self
            .run
            .parse()
            .map_err(|e| format!("Failed to parse template: {:?}", e))?;

        let mut output = String::new();
        for token in tokens {
            match token {
                Token::Lit(lit) => output.push_str(&lit),
                Token::Expr(expr) => {
                    let value = execution_ctx.eval(&expr)?;

                    let value = match value {
                        CelValue::Int(val) => val.to_string(),
                        CelValue::UInt(val) => val.to_string(),
                        CelValue::Float(val) => val.to_string(),
                        CelValue::String(val) => val.to_string(),
                        CelValue::Bool(val) => val.to_string(),
                        CelValue::Duration(val) => val.to_string(),
                        CelValue::Timestamp(val) => val.to_string(),
                        CelValue::Null => "".to_string(),
                        _ => return Err(format!("Unsupported value type: {}", value.type_of())),
                    };

                    output.push_str(&value);
                }
            }
        }

        if ctx.is_done() {
            return Err("Context is done".to_string());
        }

        let file_path = Path::new(execution_ctx.workdir()).join(Uuid::new_v4().to_string());
        {
            let mut f = match File::create(&file_path) {
                Ok(f) => f,
                Err(err) => {
                    return Err(err.to_string());
                }
            };

            if let Err(err) = f.write_all(output.as_bytes()) {
                return Err(err.to_string());
            }
        }

        Ok(file_path)
    }
}

#[derive(Debug)]
struct UploadStep {
    uploads: Vec<String>,
}

impl UploadStep {
    #[tracing::instrument(skip(ctx, execution_ctx, client))]
    fn run<C>(
        &self,
        ctx: Ctx<Background>,
        id: Uuid,
        execution_ctx: &mut ExecutionContext,
        client: &C,
        log_tx: mpsc::Sender<LogLine>,
    ) -> TimelineRequestStepState
    where
        C: JobClient,
    {
        let artifact_builder = ArtifactBuilder {
            uploads: self.uploads.clone(),
            root_dir: PathBuf::from(&execution_ctx.workdir()),
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
        match client.upload_job_artifact(
            ctx.clone(),
            execution_ctx.project_id(),
            execution_ctx.workflow_id(),
            execution_ctx.revision_id(),
            execution_ctx.job_id(),
            file,
        ) {
            Ok(_) => {
                tracing::info!("Uploaded job result");
                log_tx
                    .send(LogLine {
                        dst: LogDestination::Stdout,
                        timestamp: OffsetDateTime::now_utc(),
                        message: "Uploaded job result".to_string(),
                    })
                    .unwrap_or_else(|err| tracing::error!("Failed to send log line: {:?}", err));
                TimelineRequestStepState::Succeeded
            }
            Err(err) => {
                tracing::error!("Failed to upload job result: {:?}", err);
                log_tx
                    .send(LogLine {
                        dst: LogDestination::Stderr,
                        timestamp: OffsetDateTime::now_utc(),
                        message: format!("Failed to upload job result: {}", err),
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
        write!(f, "StepError")
    }
}

impl Context for StepError {}

#[derive(Debug)]
pub struct ExecutionError;

impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ExecutionError")
    }
}

impl Context for ExecutionError {}

#[cfg(test)]
mod tests {
    use super::*;
    use client::job::{MockJobClient, StepKind, TimelineRequestStepState};
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
        }
    }

    fn new_test_workdir() -> String {
        env::temp_dir()
            .join(Uuid::new_v4().to_string())
            .to_string_lossy()
            .to_string()
    }

    #[test]
    fn test_steps_new() {
        let resolved_steps = vec![
            JobResolvedStepResponse {
                id: Uuid::now_v7(),
                kind: StepKind::Setup,
                cond: "always".to_string(),
                run: "setup 'Hello, World!'".to_string(),
                shell: "bh".to_string(),
            },
            JobResolvedStepResponse {
                id: Uuid::now_v7(),
                kind: StepKind::Command,
                cond: "always".to_string(),
                run: "echo 'Hello, World!'".to_string(),
                shell: "sh".to_string(),
            },
            JobResolvedStepResponse {
                id: Uuid::now_v7(),
                kind: StepKind::Command,
                cond: "ok".to_string(),
                run: "echo 'Hello, World!'".to_string(),
                shell: "sh".to_string(),
            },
            JobResolvedStepResponse {
                id: Uuid::now_v7(),
                kind: StepKind::Upload,
                cond: "always".to_string(),
                run: "upload".to_string(),
                shell: "bh".to_string(),
            },
            JobResolvedStepResponse {
                id: Uuid::now_v7(),
                kind: StepKind::Teardown,
                cond: "always".to_string(),
                run: "teardown".to_string(),
                shell: "bh".to_string(),
            },
        ];

        let uploads = Some(vec!["artifact1".to_string(), "artifact2".to_string()]);

        let steps = Steps::new(resolved_steps.clone(), uploads.clone());

        assert_eq!(steps.steps.len(), resolved_steps.len());

        let setup_step = &steps.steps[0];
        assert_eq!(setup_step.id, resolved_steps[0].id);
        assert_eq!(setup_step.cond, "always");
        assert!(matches!(setup_step.inner, InnerStep::Setup));

        let command_step = &steps.steps[1];
        assert_eq!(command_step.id, resolved_steps[1].id);
        assert_eq!(command_step.cond, "always");
        assert!(matches!(
            command_step.inner,
            InnerStep::Command(CommandStep { .. })
        ));

        let command_step = &steps.steps[2];
        assert_eq!(command_step.id, resolved_steps[2].id);
        assert_eq!(command_step.cond, "ok");
        assert!(matches!(
            command_step.inner,
            InnerStep::Command(CommandStep { .. })
        ));

        let upload_step = &steps.steps[3];
        assert_eq!(upload_step.id, resolved_steps[3].id);
        assert_eq!(upload_step.cond, "always");
        assert!(matches!(
            upload_step.inner,
            InnerStep::Upload(UploadStep { .. })
        ));

        let teardown_step = &steps.steps[4];
        assert_eq!(teardown_step.id, resolved_steps[4].id);
        assert_eq!(teardown_step.cond, "always");
        assert!(matches!(teardown_step.inner, InnerStep::Teardown));
    }

    #[test]
    fn test_setup_step() {
        let step = Step {
            id: Uuid::now_v7(),
            cond: "always".to_string(),
            inner: InnerStep::Setup,
        };

        let id = step.id;
        let mut main_client = MockJobClient::new();
        main_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.id, id, "{:?}", timeline);
                assert!(
                    matches!(timeline.state, TimelineRequestStepState::Running),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        let id = step.id;
        main_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.id, id, "{:?}", timeline);
                assert!(
                    matches!(timeline.state, TimelineRequestStepState::Succeeded { .. }),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        main_client
            .expect_clone()
            .returning(MockJobClient::new)
            .times(2);

        let mut execution_ctx =
            ExecutionContext::new(new_test_workdir(), Arc::new(vec![]), new_test_job_config());

        let result = step.run(ctx::background(), &mut execution_ctx, &main_client);
        assert!(result.is_ok(), "{:?}", result);
    }

    #[test]
    fn test_teardown_step() {
        let workdir = new_test_workdir();
        let job_execution_ctx = new_test_job_config();
        let step = Step {
            id: Uuid::now_v7(),
            cond: "always".to_string(),
            inner: InnerStep::Teardown,
        };
        fs::create_dir_all(&workdir).expect("Failed to create workdir");

        let mut main_client = MockJobClient::new();
        let id = step.id;
        main_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.id, id, "{:?}", timeline);
                assert!(
                    matches!(timeline.state, TimelineRequestStepState::Running),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        let id = step.id;
        main_client
            .expect_post_step_timeline()
            .returning(move |_, timeline| {
                assert_eq!(timeline.id, id, "{:?}", timeline);
                assert!(
                    matches!(timeline.state, TimelineRequestStepState::Succeeded { .. }),
                    "{:?}",
                    timeline
                );
                Ok(())
            })
            .once();

        main_client
            .expect_clone()
            .returning(MockJobClient::new)
            .times(2);

        let mut execution_ctx = ExecutionContext::new(workdir, Arc::new(vec![]), job_execution_ctx);

        let result = step.run(ctx::background(), &mut execution_ctx, &main_client);
        assert!(result.is_ok(), "{:?}", result);
    }

    #[test]
    fn test_command_step_hello_world() {
        let step = CommandStep {
            run: "echo 'Hello, World!'".to_string(),
            shell: "sh".to_string(),
        };

        let workdir = new_test_workdir();
        fs::create_dir_all(&workdir).expect("Failed to create workdir");
        let job_execution_ctx = new_test_job_config();
        let mut execution_ctx = ExecutionContext::new(workdir, Arc::new(vec![]), job_execution_ctx);

        let (tx, rx) = mpsc::channel();
        let step_id = Uuid::now_v7();
        let result = step.run(ctx::background(), step_id, &mut execution_ctx, tx);
        assert!(
            matches!(result, TimelineRequestStepState::Succeeded),
            "{:?}",
            result
        );

        let log_line = rx.try_recv().expect("Failed to receive log line");
        assert!(matches!(log_line.dst, LogDestination::Stdout));
        assert_eq!(log_line.message, "Hello, World!");
    }

    #[test]
    fn test_command_step_env_propagation() {
        let step = CommandStep {
            run: "echo $HELLO_ENV".to_string(),
            shell: "sh".to_string(),
        };

        let workdir = new_test_workdir();
        fs::create_dir_all(&workdir).expect("Failed to create workdir");
        let job_execution_ctx = new_test_job_config();
        let mut execution_ctx = ExecutionContext::new(
            workdir,
            Arc::new(vec![("HELLO_ENV".to_string(), "Hello, World!".to_string())]),
            job_execution_ctx,
        );

        let step_id = Uuid::now_v7();
        let (tx, rx) = mpsc::channel();
        let result = step.run(ctx::background(), step_id, &mut execution_ctx, tx);
        assert!(
            matches!(result, TimelineRequestStepState::Succeeded),
            "{:?}",
            result
        );

        let log_line = rx.try_recv().expect("Failed to receive log line");
        assert!(matches!(log_line.dst, LogDestination::Stdout));
        assert_eq!(log_line.message, "Hello, World!");
    }

    #[test]
    fn test_complex_shell() {
        let step = CommandStep {
            run: "echo 'Hello, World!'".to_string(),
            shell: "bash --verbose".to_string(),
        };

        let workdir = new_test_workdir();
        fs::create_dir_all(&workdir).expect("Failed to create workdir");
        let job_execution_ctx = new_test_job_config();
        let mut execution_ctx = ExecutionContext::new(workdir, Arc::new(vec![]), job_execution_ctx);

        let step_id = Uuid::now_v7();
        let (tx, rx) = mpsc::channel();
        let result = step.run(ctx::background(), step_id, &mut execution_ctx, tx);
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
                    assert_eq!(log_line.message, "Hello, World!");
                    stdout_count += 1;
                }
                LogDestination::Stderr => {
                    assert!(log_line.message.contains("echo 'Hello, World!'"));
                    stderr_count += 1;
                }
            }
        }
        assert_eq!(stderr_count, 1);
        assert_eq!(stdout_count, 1);
    }

    #[test]
    fn test_command_cancel() {
        let step = CommandStep {
            run: "sleep 10".to_string(),
            shell: "sh".to_string(),
        };

        let workdir = new_test_workdir();
        fs::create_dir_all(&workdir).expect("Failed to create workdir");
        let job_config = new_test_job_config();
        let mut execution_ctx = ExecutionContext::new(workdir, Arc::new(vec![]), job_config);

        let background = ctx::background().with_cancel();
        let ctx = background.with_cancel();

        let step_ctx = ctx.to_background();
        let step_id = Uuid::now_v7();
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || step.run(step_ctx, step_id, &mut execution_ctx, tx));

        thread::sleep(Duration::from_millis(500));
        ctx.cancel();
        let result = handle.join().expect("failed to join step handle");
        assert!(matches!(result, TimelineRequestStepState::Cancelled));

        let log_line = rx.try_recv().expect("Failed to receive log line");
        assert!(matches!(log_line.dst, LogDestination::Stderr));
        assert!(!log_line.message.is_empty());
    }
}

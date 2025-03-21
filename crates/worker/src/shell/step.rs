use super::execution_context::ExecutionContext;
use client::job::{JobClient, TimelineRequest, TimelineRequestStepOutcome};
use client::job::{LogLine, TimelineRequestStepState};
use ctx::{Background, Ctx};
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use std::fs;
use std::path::{Component, Path, PathBuf};
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
    index: u32,
    context: &'a ExecutionContext,
    worker_client: C,
}

impl<C> Step for SetupStep<'_, C>
where
    C: JobClient,
{
    #[tracing::instrument(skip(self, ctx))]
    fn run(&self, ctx: Ctx<Background>) -> Result<bool> {
        tracing::debug!("Executing setup step");
        let workdir = self.context.workdir();

        let timeline_request = TimelineRequest {
            index: self.index,
            state: TimelineRequestStepState::Running,
        };
        tracing::debug!("Posting step state: {timeline_request:?}");
        self.worker_client
            .post_step_timeline(ctx.clone(), &timeline_request)
            .wrap_err("Failed to post step timeline")?;

        tracing::debug!("Creating workdir '{workdir}'");
        match fs::create_dir_all(workdir) {
            Ok(_) => {
                let msg = format!("Sucessfully created workdir '{workdir}'");
                tracing::debug!("{msg}");
                self.worker_client
                    .send_job_logs(ctx.clone(), vec![LogLine::stdout(self.index, &msg)])
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
                let msg = format!("Failed to create workdir '{workdir}': {e:?}");
                tracing::debug!("{msg}");
                self.worker_client
                    .send_job_logs(ctx.clone(), vec![LogLine::stderr(self.index, &msg)])
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
    index: u32,
    context: &'a ExecutionContext,
    worker_client: C,
}

impl<C> Step for TeardownStep<'_, C>
where
    C: JobClient,
{
    #[tracing::instrument(skip(self, ctx))]
    fn run(&self, ctx: Ctx<Background>) -> Result<bool> {
        tracing::debug!("Executing teardown step");
        let workdir = self.context.workdir();

        let timeline_request = TimelineRequest {
            index: self.index,
            state: TimelineRequestStepState::Running,
        };
        tracing::debug!("Posting step state: {timeline_request:?}");
        self.worker_client
            .post_step_timeline(ctx.clone(), &timeline_request)
            .wrap_err("Failed to post step timeline")?;

        tracing::debug!("Creating workdir '{workdir}'");
        match fs::remove_dir_all(workdir) {
            Ok(_) => {
                let msg = format!("Sucessfully removed workdir '{workdir}'");
                tracing::debug!("{msg}");
                self.worker_client
                    .send_job_logs(ctx.clone(), vec![LogLine::stdout(self.index, &msg)])
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
                let msg = format!("Failed to remove workdir '{workdir}': {e:?}");
                tracing::debug!("{msg}");
                self.worker_client
                    .send_job_logs(ctx.clone(), vec![LogLine::stderr(self.index, &msg)])
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

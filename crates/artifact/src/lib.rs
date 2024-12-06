use ctx::{Background, Ctx};
use error_stack::{Context, Report, Result, ResultExt};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::{fmt, fs, io};
use uuid::Uuid;
use zip::write::{FileOptionExtension, FileOptions, SimpleFileOptions};
use zip::ZipWriter;

#[derive(Debug)]
pub struct ArtifactBuilder {
    pub uploads: Vec<String>,
    pub root_dir: PathBuf,
}

impl ArtifactBuilder {
    #[tracing::instrument(skip(ctx))]
    pub fn build(&self, ctx: Ctx<Background>) -> Result<Artifact, ArtifactError> {
        let filename = format!("{}.zip", Uuid::new_v4());
        let result_path = self.root_dir.join(filename);

        let result = File::create(result_path.clone()).map_err(|e| ArtifactError(e.to_string()))?;

        let mut zip = ZipWriter::new(result);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(9));

        tracing::debug!("Zipping paths: {:?}", self.uploads);
        let root_dir = self
            .root_dir
            .canonicalize()
            .change_context(ArtifactError("Canonize error".to_string()))
            .attach_printable("Failed to canonize root directory")?;
        for path in &self.uploads {
            if ctx.is_done() {
                return Err(Report::new(ArtifactError("Cancelled".to_string()))
                    .attach_printable("context is done"));
            }
            zip_path(ctx.clone(), &mut zip, &root_dir, path, options)
                .map_err(|e| ArtifactError(e.to_string()))?;
        }

        tracing::debug!("Finishing zip");
        zip.finish().map_err(|e| ArtifactError(e.to_string()))?;

        Ok(Artifact { file: result_path })
    }
}

#[tracing::instrument(skip(w, options))]
fn zip_path<T: FileOptionExtension + Copy>(
    ctx: Ctx<Background>,
    w: &mut ZipWriter<File>,
    root_dir: &PathBuf,
    path: &str,
    options: FileOptions<T>,
) -> Result<(), ArtifactError> {
    if ctx.is_done() {
        return Err(
            Report::new(ArtifactError("Cancelled".to_string())).attach_printable("context is done")
        );
    }

    let path = Path::new(&root_dir)
        .join(path)
        .canonicalize()
        .change_context(ArtifactError("Canonize error".to_string()))
        .attach_printable("Failed to canonize path")?;

    if !path.starts_with(root_dir) {
        return Err(Report::new(ArtifactError(
            "Path is outside of root directory".to_string(),
        ))
        .attach_printable(format!(
            "path: '{}', root_dir: '{}'",
            path.display(),
            root_dir.display()
        )));
    }

    tracing::debug!("Zipping path: {:?}", path);
    if path.is_symlink() {
        return Err(Report::new(ArtifactError(format!(
            "Symlinks are not supported(path: '{}')",
            path.display()
        ))));
    }

    if path.is_dir() {
        let span = tracing::info_span!("Zipping directory", path = ?path);
        let _guard = span.enter();

        tracing::info!("Reading directory entries");
        for entry in fs::read_dir(path.clone()).map_err(|e| ArtifactError(e.to_string()))? {
            let entry = entry.map_err(|e| ArtifactError(e.to_string()))?;

            let path = entry.path();
            tracing::debug!("Entry path: {:?}", path);

            if path.is_symlink() {
                tracing::info!("Path is symlink, skipping");
                continue;
            }

            if path.is_dir() {
                tracing::info!("Path is directory, recursing into directory");
                zip_path(
                    ctx.clone(),
                    w,
                    root_dir,
                    path.strip_prefix(root_dir).unwrap().to_str().unwrap(),
                    options,
                )?;
            } else {
                tracing::info!("Path is file, copying file");
                let name = path.strip_prefix(root_dir).unwrap();
                w.start_file(name.to_str().unwrap(), options)
                    .map_err(|e| ArtifactError(e.to_string()))?;

                tracing::info!("Opening file: {:?}", name);
                let mut f = File::open(&path).map_err(|e| ArtifactError(e.to_string()))?;

                tracing::info!("Copying file: {:?}", name);
                io::copy(&mut f, w).map_err(|e| ArtifactError(e.to_string()))?;
            }
        }
    } else {
        tracing::info!("Path is file, copying file");
        w.start_file(
            path.strip_prefix(root_dir).unwrap().to_str().unwrap(),
            options,
        )
        .map_err(|e| ArtifactError(e.to_string()))?;

        tracing::info!("Opening file: {:?}", path);
        let mut f = File::open(path.clone()).map_err(|e| ArtifactError(e.to_string()))?;

        tracing::info!("Copying file: {:?}", path);
        io::copy(&mut f, w).map_err(|e| ArtifactError(e.to_string()))?;
    }

    Ok(())
}

#[derive(Debug)]
pub struct Artifact {
    pub file: PathBuf,
}

#[derive(Debug)]
pub struct ArtifactError(String);

impl fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Artifact error: {}", self.0)
    }
}

impl Context for ArtifactError {}

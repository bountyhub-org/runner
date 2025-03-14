use ctx::{Background, Ctx};
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use std::fs::File;
use std::path::{Component, Path, PathBuf};
use std::{fs, io};
use uuid::Uuid;
use zip::write::{FileOptionExtension, FileOptions, SimpleFileOptions};
use zip::ZipWriter;

#[derive(Debug)]
pub struct ArtifactBuilder {
    uploads: Vec<PathBuf>,
    root_dir: PathBuf,
}

fn normalize_to_relative_path(root: &PathBuf, s: &str) -> Result<PathBuf> {
    let path = Path::new(s);
    let mut components = path.components().peekable();
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
                bail!("upload record {s} cannot be taken from the root");
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !ret.pop() {
                    bail!("upload record {s} cannot be outside of the present working directory");
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
        bail!("path '{s}' after resolution to '{canonized:?}' doesn't start with {root:?}",)
    }

    Ok(canonized)
}

impl ArtifactBuilder {
    pub fn new(root_dir: &Path, uploads: Vec<String>) -> Result<Self> {
        if uploads.is_empty() {
            bail!("Uploads is empty");
        }

        let root = root_dir
            .canonicalize()
            .into_diagnostic()
            .wrap_err("failed to canonicalize root dir '{root_dir:?}'")?;

        let mut output = Vec::with_capacity(uploads.len());
        for upload in uploads {
            output.push(
                normalize_to_relative_path(&root, upload.as_str())
                    .wrap_err("failed to canonize upload {upload}")?,
            );
        }

        Ok(Self {
            root_dir: root,
            uploads: output,
        })
    }

    #[tracing::instrument(skip(ctx))]
    pub fn build(&self, ctx: Ctx<Background>) -> Result<Artifact> {
        let filename = format!("{}.zip", Uuid::new_v4());
        let result_path = self.root_dir.join(filename);

        let result = File::create(result_path.clone()).into_diagnostic()?;

        let mut zip = ZipWriter::new(result);
        let options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(9));

        tracing::debug!("Zipping paths: {:?}", self.uploads);
        for path in &self.uploads {
            if ctx.is_done() {
                bail!("Context is cancelled");
            }
            self.zip_path(ctx.clone(), &mut zip, path, options)?;
        }

        tracing::debug!("Finishing zip");
        zip.finish().into_diagnostic()?;

        Ok(Artifact { file: result_path })
    }

    #[tracing::instrument(skip(w, options))]
    fn zip_path<T: FileOptionExtension + Copy>(
        &self,
        ctx: Ctx<Background>,
        w: &mut ZipWriter<File>,
        path: &PathBuf,
        options: FileOptions<T>,
    ) -> Result<()> {
        if ctx.is_done() {
            bail!("Context is cancelled");
        }

        if path.is_dir() {
            let span = tracing::info_span!("Zipping directory", path = ?path);
            let _guard = span.enter();

            tracing::info!("Reading directory entries");
            for entry in fs::read_dir(path.clone()).into_diagnostic()? {
                let entry = entry.into_diagnostic()?;

                let path = entry.path();
                tracing::debug!("Entry path: {:?}", path);

                if path.is_symlink() {
                    tracing::info!("Path is symlink, skipping");
                    continue;
                }

                if path.is_dir() {
                    tracing::info!("Path is directory, recursing into directory");
                    self.zip_path(
                        ctx.clone(),
                        w,
                        &path.strip_prefix(&self.root_dir).unwrap().to_path_buf(),
                        options,
                    )?;
                } else {
                    tracing::info!("Path is file, copying file");
                    w.start_file_from_path(
                        path.strip_prefix(&self.root_dir).into_diagnostic()?,
                        options,
                    )
                    .into_diagnostic()?;

                    tracing::info!("Opening file: {path:?}");
                    let mut f = File::open(self.root_dir.join(&path)).into_diagnostic()?;

                    tracing::info!("Copying file: {path:?}");
                    io::copy(&mut f, w).into_diagnostic()?;
                }
            }
        } else {
            tracing::info!("Path is file, copying file");
            w.start_file_from_path(
                path.strip_prefix(&self.root_dir).into_diagnostic()?,
                options,
            )
            .into_diagnostic()?;

            tracing::info!("Opening file: {path:?}");
            let mut f = File::open(self.root_dir.join(path)).into_diagnostic()?;

            tracing::info!("Copying file: {path:?}");
            io::copy(&mut f, w).into_diagnostic()?;
        }

        Ok(())
    }
}

#[derive(Debug)]
pub struct Artifact {
    pub file: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;
    use io::Read;
    use std::{env, fs::File, io::Write};
    use uuid::Uuid;
    use zip::ZipArchive;

    struct FolderCleaner {
        base_path: PathBuf,
    }

    impl Drop for FolderCleaner {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.base_path).expect("removal to occur");
        }
    }

    #[test]
    fn test_normalize_to_relative_path() {
        let testdir = env::temp_dir().join(Uuid::new_v4().to_string());
        let workdir = testdir.join(Uuid::new_v4().to_string());
        fs::create_dir_all(&workdir).expect("create test and workdir");
        let _cleaner = FolderCleaner {
            base_path: testdir.clone(),
        };

        let temp_filename = Uuid::new_v4().to_string();
        let temp_outfile = testdir.join(&temp_filename);
        {
            File::create(&temp_outfile).expect("to create temp outfile");
        }
        let inner_file = workdir.join(Uuid::new_v4().to_string());
        {
            File::create(&inner_file).expect("to create temp outfile");
        }
        let abs_temp = temp_outfile
            .canonicalize()
            .expect("canonicalize should succeed");

        assert!(
            normalize_to_relative_path(&workdir, abs_temp.to_str().unwrap()).is_err(),
            "expected abs path to return an error"
        );

        let out_of_workdir = format!("{}/../temp_filename", temp_outfile.to_str().unwrap());
        assert!(
            normalize_to_relative_path(&workdir, out_of_workdir.as_str()).is_err(),
            "expected relative path out of workdir to file that exists to fail"
        );

        assert!(
            normalize_to_relative_path(
                &workdir,
                workdir.join(Uuid::new_v4().to_string()).to_str().unwrap(),
            )
            .is_err(),
            "expected file resolution that doesn't exist in workdir to faile",
        );

        let result = normalize_to_relative_path(
            &workdir,
            inner_file
                .strip_prefix(&workdir)
                .expect("strip to be ok")
                .to_str()
                .unwrap(),
        );
        assert!(
            result.is_ok(),
            "unexpected error normalizing relative path in workdir: {result:?}"
        );
    }

    #[test]
    fn test_build_artifact() {
        let workdir = env::temp_dir().join(Uuid::new_v4().to_string());

        let dir_name = Uuid::new_v4().to_string();
        fs::create_dir_all(workdir.join(&dir_name)).expect("directories should be created");

        let _cleaner = FolderCleaner {
            base_path: workdir.clone(),
        };

        let root_file_path = Uuid::new_v4().to_string();
        {
            let file_path = workdir.join(&root_file_path);
            let mut f = File::create(file_path).expect("file should be created");
            f.write_all(b"root file").expect("write succeed");
        }

        let dir_file_path = Path::new(&dir_name)
            .join(Uuid::new_v4().to_string())
            .to_string_lossy()
            .to_string();

        {
            let file_path = workdir.join(&dir_file_path);
            let mut f = File::create(file_path).expect("file should be created");
            f.write_all(b"inner file").expect("write succeed");
        }

        let builder = ArtifactBuilder::new(
            &workdir,
            vec![root_file_path.clone(), dir_file_path.clone()],
        )
        .expect("artifact builder to be created");

        let artifact = builder
            .build(ctx::background())
            .expect("build to be successful");

        let result_dir = workdir.join("result");
        {
            let f = File::open(artifact.file).expect("zip file to be created");
            let mut archive = ZipArchive::new(f).expect("zip archive to be instantiated");
            archive.extract(&result_dir).expect("extract to be ok");
        }

        {
            let mut f =
                File::open(result_dir.join(&root_file_path)).expect("root file should exist");
            let mut buf = String::new();
            f.read_to_string(&mut buf).expect("read to succeed");

            assert_eq!("root file", buf);
        }
        {
            let mut f = File::open(result_dir.join(dir_file_path)).expect("open dir file path");
            let mut buf = String::new();
            f.read_to_string(&mut buf).expect("read to succeed");

            assert_eq!("inner file", buf);
        }
    }
}

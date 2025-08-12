use miette::{Diagnostic, IntoDiagnostic, LabeledSpan, Result, WrapErr, bail, miette};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, RwLock};
use std::{env, fs, io};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

pub const CONFIG_FILE: &str = ".runner";
pub const RUNNER_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const VERSION: &str = env!("BUILD_VERSION");

/// Manages the configuration by reading/writing and caching it internally
/// If the configuration file changes, it doesn't matter. We source it once,
/// and keep it in memory and writing through.
#[derive(Debug, Clone, Default)]
pub struct ConfigManager {
    name: String,
    inner: Arc<RwLock<Option<Config>>>,
}

impl ConfigManager {
    pub fn new(name: String) -> Self {
        Self {
            name,
            inner: Arc::new(RwLock::new(None)),
        }
    }

    pub fn get(&self) -> Result<Config> {
        {
            let c = self.inner.read().expect("read lock to succeed");
            if let Some(ref c) = *c {
                return Ok(c.clone());
            }
        }

        let runner_home = runner_home(self.name.as_str())?;

        let mut c = self.inner.write().expect("write lock to succeed");
        let path = runner_home.join(CONFIG_FILE);
        let config: Config =
            serde_json::from_str(&fs::read_to_string(&path).into_diagnostic().wrap_err(
                format!("Failed to read config file {path:?} from present working directory"),
            )?)
            .into_diagnostic()
            .wrap_err("Failed to deserialize configuration")?;

        config.validate().wrap_err("Invalid configuration")?;

        *c = Some(config.clone());
        Ok(config)
    }

    pub fn put(&self, cfg: &Config) -> Result<()> {
        cfg.validate()?;
        let mut c = self.inner.write().expect("lock to succeed");
        let content = serde_json::to_string(cfg)
            .into_diagnostic()
            .wrap_err("Failed to serialize configuration")?;

        let home = runner_home(&cfg.name)?;

        fs::create_dir_all(&home)
            .into_diagnostic()
            .wrap_err("Failed to create .bountyhub directory")?;

        let config_dir = home.join(CONFIG_FILE);

        fs::write(config_dir, content)
            .into_diagnostic()
            .wrap_err("Failed to write config to a file")?;

        *c = Some(cfg.clone());
        Ok(())
    }
}

pub fn bountyhub_home() -> Result<PathBuf> {
    Ok(env::home_dir()
        .ok_or_else(|| miette!("Failed to get home directory"))?
        .join(".bountyhub"))
}

pub fn runner_home(name: &str) -> Result<PathBuf> {
    Ok(bountyhub_home()?.join("runner").join(name))
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct Config {
    pub token: String,
    pub hub_url: String,
    pub invoker_url: String,
    pub fluxy_url: String,
    pub name: String,
    pub workdir: PathBuf,
    pub capacity: u32,
}

impl Config {
    #[tracing::instrument]
    pub fn validate(&self) -> Result<()> {
        validate_token(&self.token).wrap_err("Token validation error")?;
        validate_url(&self.hub_url).wrap_err("URL validation error")?;
        validate_url(&self.invoker_url).wrap_err("URL validation error")?;
        validate_url(&self.fluxy_url).wrap_err("URL validation error")?;
        validate_name(&self.name).wrap_err("Name validation error")?;
        validate_capacity(self.capacity).wrap_err("Capacity validation error")?;
        Ok(())
    }
}

#[tracing::instrument(skip(token))]
pub fn validate_token(token: &str) -> Result<()> {
    if token.len() < 10 {
        return Err(miette!("Token is too short").wrap_err(Error::ValidationError));
    }
    Ok(())
}

pub fn validate_workdir(workdir: &PathBuf) -> Result<()> {
    if workdir.is_file() {
        bail!("Workdir {workdir:?} is file");
    }

    Ok(())
}

#[tracing::instrument(skip(workdir))]
pub fn validate_workdir_str(workdir: &str) -> Result<()> {
    PathBuf::from_str(workdir)
        .into_diagnostic()
        .wrap_err("Failed to parse workdir as path")?;
    Ok(())
}

#[tracing::instrument]
pub fn validate_url(url: &str) -> Result<()> {
    let url = Url::parse(url)
        .into_diagnostic()
        .wrap_err(Error::ValidationError)
        .wrap_err("Failed to parse url")?;

    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(
            miette!("Invalid url scheme: must be http or https").wrap_err(Error::ValidationError)
        );
    }
    if !url.username().is_empty() {
        return Err(
            miette!("Invalid url: username is not allowed").wrap_err(Error::ValidationError)
        );
    }
    if !url.has_host() {
        return Err(miette!("Invalid url: host is required").wrap_err(Error::ValidationError));
    }

    Ok(())
}

#[tracing::instrument]
pub fn validate_name(name: &str) -> Result<()> {
    const RUNNER_NAME_CONSTRAINT: &str = "name must contain alphanumeric characters or dashes and be between 3 and 50 characters long";

    if !(3..=50).contains(&name.len()) {
        return Err(miette!("Name length is invalid: {RUNNER_NAME_CONSTRAINT}")
            .wrap_err(Error::ValidationError));
    }

    for (i, c) in name.chars().enumerate() {
        if !c.is_ascii_alphanumeric() && c != '-' {
            return Err(miette! {
                labels = vec![LabeledSpan::at(
                    i..i+1,
                    "Invalid character"
                )],
                help = format!("Runner name must be alphanumeric or '-'"),
                "Invalid character as a name"
            }
            .with_source_code(name.to_string())
            .wrap_err(Error::ValidationError));
        }
    }

    Ok(())
}

#[tracing::instrument]
pub fn validate_capacity(capacity: u32) -> Result<()> {
    if !(1..=100).contains(&capacity) {
        return Err(miette!("Capacity must be between 1 and 100").wrap_err(Error::ValidationError));
    }
    Ok(())
}

#[derive(Diagnostic, Debug, Error)]
#[diagnostic(code(client::client_error))]
pub enum Error {
    #[error("validation error")]
    ValidationError,

    #[error(transparent)]
    IoError(#[from] io::Error),

    #[error(transparent)]
    SerdeError(#[from] serde_json::Error),
}

#[tracing::instrument]
pub fn generate_default_name() -> String {
    hostname::get().map_or_else(
        |_| Uuid::new_v4().to_string(),
        |host| {
            let host = host.to_string_lossy().to_string();
            match validate_name(&host) {
                Ok(_) => host,
                Err(_) => Uuid::new_v4().to_string(),
            }
        },
    )
}

#[test]
fn test_runner_name_validation() {
    assert!(validate_name("test-123-t3s7").is_ok());
    assert!(validate_name("test 123").is_err());
    assert!(validate_name("test_123").is_err());
    assert!(validate_name("ab").is_err());
    assert!(validate_name("a".repeat(51).as_str()).is_err());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, path::PathBuf};

    #[derive(Debug, Clone)]
    struct TestDir {
        dir: PathBuf,
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            if let Err(e) = fs::remove_dir_all(&self.dir) {
                eprintln!("Failed to remove test directory: {e:?}");
            }
        }
    }

    impl TestDir {
        fn init() -> Self {
            let dir = env::temp_dir().join(Uuid::new_v4().to_string());
            fs::create_dir_all(&dir).expect("create dir all should be ok");
            assert!(dir.exists(), "Directory {dir:?} should exist");

            TestDir { dir }
        }
    }

    #[test]
    fn test_config_manager_put_and_get_ok() {
        let test_dir = TestDir::init();
        let cfg = Config {
            token: Uuid::new_v4().to_string(),
            hub_url: "https://bountyhub.org".to_string(),
            invoker_url: "https://invoker.bountyhub.org".to_string(),
            fluxy_url: "https://fluxy.bountyhub.org".to_string(),
            name: "test".to_string(),
            workdir: test_dir.dir.clone(),
            capacity: 1,
        };
        let cm = ConfigManager::new("test".to_string());
        cm.put(&cfg)
            .unwrap_or_else(|e| panic!("to save config to directory {test_dir:?}: {e:?}"));
        let got = cm.get().expect("to get the config");
        assert_eq!(cfg, got);
    }
}

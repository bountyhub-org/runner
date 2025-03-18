use std::sync::Arc;

use miette::{miette, Diagnostic, IntoDiagnostic, LabeledSpan, Result, WrapErr};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use url::Url;
use uuid::Uuid;

pub const CONFIG_FILE: &str = ".runner";
pub const RUNNER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Default)]
pub struct ConfigManager {
    inner: Arc<tokio::sync::Mutex<Option<Config>>>,
}

impl ConfigManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get(&self) -> Result<Config> {
        let mut config = self.inner.lock().await;
        match *config {
            Some(ref cfg) => {
                cfg.validate()?;
                Ok(cfg.clone())
            }
            None => {
                let cfg = read().await?;
                cfg.validate()?;
                *config = Some(cfg.clone());
                Ok(cfg)
            }
        }
    }

    pub async fn put(&self, cfg: &Config) -> Result<()> {
        cfg.validate()?;
        let mut config = self.inner.lock().await;
        write(cfg).await?;
        *config = Some(cfg.to_owned());
        Ok(())
    }
}

#[tracing::instrument]
async fn read() -> Result<Config> {
    let content = fs::read_to_string(CONFIG_FILE).await.map_err(Error::from)?;

    let config: Config = serde_json::from_slice(content.as_bytes()).map_err(Error::from)?;

    config.validate()?;

    fs::create_dir_all(&config.workdir)
        .await
        .map_err(Error::from)
        .wrap_err("Failed to create workdir")?;

    Ok(config)
}

#[tracing::instrument]
pub async fn write(cfg: &Config) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(CONFIG_FILE)
        .await
        .map_err(Error::from)?;

    let content = serde_json::to_string(cfg).map_err(Error::from)?;

    file.write_all(content.as_bytes())
        .await
        .map_err(Error::from)?;

    Ok(())
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Config {
    pub token: String,
    pub hub_url: String,
    pub invoker_url: String,
    pub fluxy_url: String,
    pub name: String,
    pub workdir: String,
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
        validate_workdir(&self.workdir).wrap_err("Workdir validation error")?;
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
    const RUNNER_NAME_CONSTRAINT: &str =
    "name must contain alphanumeric characters or dashes and be between 3 and 50 characters long";

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
pub fn validate_workdir(workdir: &str) -> Result<()> {
    if workdir.is_empty() {
        return Err(miette!("Workdir is empty").wrap_err(Error::ValidationError));
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
    IoError(#[from] tokio::io::Error),

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

use error_stack::{Context, Report, Result, ResultExt};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::{fmt, fs};
use url::Url;
use uuid::Uuid;

pub const CONFIG_FILE: &str = ".runner";
pub const RUNNER_VERSION: &str = env!("CARGO_PKG_VERSION");

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
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_token(&self.token).attach_printable("Token validation error")?;
        validate_url(&self.hub_url).attach_printable("URL validation error")?;
        validate_url(&self.invoker_url).attach_printable("URL validation error")?;
        validate_url(&self.fluxy_url).attach_printable("URL validation error")?;
        validate_name(&self.name).attach_printable("Name validation error")?;
        validate_workdir(&self.workdir).attach_printable("Workdir validation error")?;
        validate_capacity(self.capacity).attach_printable("Capacity validation error")?;
        Ok(())
    }

    #[tracing::instrument]
    pub fn write(&self) -> Result<(), ConfigurationError> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(CONFIG_FILE)
            .attach_printable("failed to open config file")
            .change_context(ConfigurationError)?;

        let content = serde_json::to_string(&self)
            .attach_printable("serde json error")
            .change_context(ConfigurationError)?;

        file.write_all(content.as_bytes())
            .attach_printable("failed to write config")
            .change_context(ConfigurationError)?;

        Ok(())
    }

    #[tracing::instrument]
    pub fn read() -> Result<Config, ConfigurationError> {
        let mut file = fs::OpenOptions::new()
            .read(true)
            .open(CONFIG_FILE)
            .attach_printable("failed to open config file")
            .change_context(ConfigurationError)?;

        let content = &mut String::new();
        file.read_to_string(content)
            .attach_printable("read content error")
            .change_context(ConfigurationError)?;

        let config: Config = serde_json::from_slice(content.as_bytes())
            .attach_printable("reading content failed from json")
            .change_context(ConfigurationError)?;

        config
            .validate()
            .change_context(ConfigurationError)
            .attach_printable("Invalid configuration")?;

        fs::create_dir_all(&config.workdir)
            .attach_printable("failed to create workdir")
            .change_context(ConfigurationError)?;

        Ok(config)
    }
}

#[tracing::instrument(skip(token))]
pub fn validate_token(token: &str) -> Result<(), ValidationError> {
    if token.len() < 10 {
        return Err(Report::new(ValidationError).attach_printable("Token is too short"));
    }
    Ok(())
}

#[tracing::instrument]
pub fn validate_url(url: &str) -> Result<(), ValidationError> {
    let url = Url::parse(url)
        .change_context(ValidationError)
        .attach_printable("failed to parse url")?;

    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(Report::new(ValidationError)
            .attach_printable("Invalid url scheme: must be http or https"));
    }
    if !url.username().is_empty() {
        return Err(
            Report::new(ValidationError).attach_printable("Invalid url: username is not allowed")
        );
    }
    if !url.has_host() {
        return Err(Report::new(ValidationError).attach_printable("Invalid url: host is required"));
    }

    Ok(())
}

#[tracing::instrument]
pub fn validate_name(name: &str) -> Result<(), ValidationError> {
    const RUNNER_NAME_CONSTRAINT: &str =
    "name must contain alphanumeric characters or dashes and be between 3 and 50 characters long";

    if !(3..=50).contains(&name.len()) {
        return Err(Report::new(ValidationError).attach_printable(format!(
            "Name length is invalid: {}",
            RUNNER_NAME_CONSTRAINT
        )));
    }

    for c in name.chars() {
        if !c.is_ascii_alphanumeric() && c != '-' {
            return Err(Report::new(ValidationError).attach_printable(format!(
                "Character in name is invalid: {}",
                RUNNER_NAME_CONSTRAINT
            )));
        }
    }

    Ok(())
}

#[tracing::instrument]
pub fn validate_workdir(workdir: &str) -> Result<(), ValidationError> {
    if workdir.is_empty() {
        return Err(Report::new(ValidationError).attach_printable("Workdir is empty"));
    }
    Ok(())
}

#[tracing::instrument]
pub fn validate_capacity(capacity: u32) -> Result<(), ValidationError> {
    if !(1..=100).contains(&capacity) {
        return Err(
            Report::new(ValidationError).attach_printable("Capacity must be between 1 and 100")
        );
    }
    Ok(())
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

#[derive(Debug)]
pub struct ConfigurationError;

impl fmt::Display for ConfigurationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Config error")
    }
}

impl Context for ConfigurationError {}

#[derive(Debug)]
pub struct ValidationError;

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Validation error")
    }
}

impl Context for ValidationError {}

#[derive(Debug)]
pub struct RunError;

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Run error")
    }
}

impl Context for RunError {}

#[test]
fn test_runner_name_validation() {
    assert!(validate_name("test-123-t3s7").is_ok());
    assert!(validate_name("test 123").is_err());
    assert!(validate_name("test_123").is_err());
    assert!(validate_name("ab").is_err());
    assert!(validate_name("a".repeat(51).as_str()).is_err());
}

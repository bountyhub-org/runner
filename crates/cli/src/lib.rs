use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use client::client_set::ClientSet;
use client::registration::{RegistrationClient, RegistrationRequest};
use client::runner::JobAcquiredResponse;
use client::worker::HttpWorkerClient;
use ctx::{Background, Ctx};
use miette::{IntoDiagnostic, Result, WrapErr, bail};
use runner::Runner;
use std::io::{self, Write};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use sudoservice::service::Service;
use sudoservice::systemd::{Config as SystemdConfig, Systemd};
use worker::shell::ShellWorker;

pub(crate) mod prompt;

use config::{self, Config, ConfigManager};

#[derive(Parser, Debug)]
#[command(
    author = "BountyHub",
    version = env!("CARGO_PKG_VERSION"),
    name = "runner",
    about = "Runner executing jobs for BountyHub platform"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    #[command(arg_required_else_help = true, about = "Configure runner")]
    Configure {
        #[arg(short, long)]
        token: String,

        #[arg(short, long)]
        url: String,

        #[arg(short, long)]
        name: Option<String>,

        #[arg(short, long, default_value = "_work")]
        workdir: String,

        #[arg(short, long, default_value = "1")]
        capacity: u32,

        #[arg(long, default_value = "false")]
        unattended: bool,
    },
    #[command(about = "Run runner in foreground.")]
    Run {},
    #[command(about = "Manage runner service")]
    Service {
        #[clap(subcommand)]
        action: ServiceCommands,
    },
    #[command(arg_required_else_help = true)]
    Completion {
        #[arg(value_enum)]
        shell: Shell,
    },
}

#[derive(Debug, Subcommand)]
enum ServiceCommands {
    #[command(about = "Install runner as a service")]
    Install {},
    #[command(about = "Start runner service")]
    Start {},
    #[command(about = "Stop runner service")]
    Stop {},
    #[command(about = "Uninstall runner service")]
    Uninstall {},
    #[command(about = "Restart runner service")]
    Restart {},
    #[command(about = "Check runner service status")]
    Status {},
}

impl Cli {
    pub fn run(self, ctx: Ctx<Background>) -> Result<()> {
        let client_set = ClientSet::default();
        let config_manager = ConfigManager::new();

        match self.command {
            Commands::Configure {
                token,
                url,
                name,
                workdir,
                capacity,
                unattended,
            } => {
                let name = match name {
                    Some(name) => name,
                    None => {
                        if unattended {
                            config::generate_default_name()
                        } else {
                            prompt::runner_name().wrap_err("failed to prompt for runner name")?
                        }
                    }
                };

                config::validate_name(&name).wrap_err("Invalid name")?;
                config::validate_url(&url).wrap_err("Invalid URL")?;
                config::validate_token(&token).wrap_err("Invalid token")?;
                config::validate_workdir_str(&workdir).wrap_err("Invalid workdir")?;
                config::validate_capacity(capacity).wrap_err("Invalid capacity")?;

                let request = RegistrationRequest {
                    name,
                    token,
                    workdir,
                };

                let client = client_set.registration_client(&url);

                let response = client
                    .register(ctx, &request)
                    .wrap_err("Failed to register runner")?;

                let config = Config {
                    token: response.token,
                    hub_url: url,
                    invoker_url: response.invoker_url,
                    fluxy_url: response.fluxy_url,
                    name: request.name,
                    workdir: PathBuf::from_str(&request.workdir)
                        .into_diagnostic()
                        .wrap_err("Failed to parse workdir")?,
                    capacity,
                };

                config
                    .validate()
                    .wrap_err("Failed to validate configuration after registration")?;
                config_manager
                    .put(&config)
                    .wrap_err("Failed to put configuration")?;

                tracing::info!("Configuration saved to {}", config::CONFIG_FILE);

                Ok(())
            }
            Commands::Service { action } => {
                let cfg = config_manager
                    .get()
                    .wrap_err("Failed to get configuration")?;

                let svc_name = format!("org.bountyhub.runner.{}", cfg.name);

                if !Systemd::is_available() {
                    bail!("Systemd is not available on this system");
                }

                let worker_envs: Vec<(String, String)> = dotenv::vars().collect();

                let cfg = SystemdConfig {
                    name: svc_name.clone(),
                    display_name: "BountyHub Runner".to_string(),
                    description: Some("BountyHub Runner started as a service".to_string()),
                    username: None,
                    executable: std::env::current_exe()
                        .into_diagnostic()
                        .wrap_err("Failed to get current executable path")?,
                    args: Some(vec!["run".to_string()]),
                    working_directory: Some(
                        std::env::current_dir()
                            .into_diagnostic()
                            .wrap_err("Failed to get current working directory")?,
                    ),
                    environment: Some(worker_envs),
                    ch_root: None,
                    restart: true,
                    restart_sec: None,
                };

                let manager = Systemd::new(cfg);

                match action {
                    ServiceCommands::Install {} => {
                        tracing::debug!("Installing service: {}", svc_name);

                        manager
                            .install()
                            .into_diagnostic()
                            .wrap_err("Failed to install service.")?;

                        tracing::info!("Service '{}' installed successfully", svc_name);
                    }
                    ServiceCommands::Start {} => {
                        tracing::debug!("Starting service: {}", svc_name);
                        manager.start().into_diagnostic().wrap_err(
                            "Failed to start service. Please make sure the service is installed",
                        )?;
                        tracing::info!("Service '{}' started successfully", svc_name);
                    }
                    ServiceCommands::Stop {} => {
                        tracing::debug!("Stopping service: {}", svc_name);
                        manager.stop().into_diagnostic().wrap_err(
                            "Failed to stop service. Please make sure the service is installed",
                        )?;
                        tracing::info!("Service '{}' stopped successfully", svc_name);
                    }
                    ServiceCommands::Uninstall {} => {
                        tracing::debug!("Uninstalling service: {}", svc_name);
                        manager
                            .uninstall()
                            .into_diagnostic()
                            .wrap_err(
                                "Failed to uninstall service. Please make sure the service is installed",
                            )?;
                        tracing::info!("Service '{}' uninstalled successfully", svc_name);
                    }
                    ServiceCommands::Restart {} => {
                        tracing::debug!("Restarting service: {}", svc_name);
                        manager.restart().into_diagnostic().wrap_err(
                            "Failed to restart service. Please make sure the service is installed",
                        )?;
                        tracing::info!("Service '{}' restarted successfully", svc_name);
                    }
                    ServiceCommands::Status {} => {
                        tracing::debug!("Checking service status: {}", svc_name);
                        let status = manager
                            .status()
                            .into_diagnostic()
                            .wrap_err(
                            "Failed to get service status. Please make sure the service is installed",
                        )?;
                        tracing::info!("Service '{}' status: {:?}", svc_name, status);
                    }
                }
                Ok(())
            }
            Commands::Run {} => {
                config_manager.get().wrap_err("Failed to get config")?;

                let worker_envs: Arc<Vec<(String, String)>> = Arc::new(dotenv::vars().collect());

                let worker_builder = WorkerBuilder {
                    config: config_manager.clone(),
                    client_set: client_set.clone(),
                    envs: Arc::clone(&worker_envs),
                };

                let runner_client = client_set.runner_client();
                let runner = Runner::new(config_manager.clone(), worker_builder);

                runner
                    .run(ctx.clone(), runner_client)
                    .wrap_err("runner exited with error")?;

                Ok(())
            }
            Commands::Completion { shell } => {
                let mut cmd = Self::command();
                let name = cmd.get_name().to_string();
                let mut stdout = io::stdout().lock();
                generate(shell, &mut cmd, name, &mut stdout);
                stdout
                    .flush()
                    .into_diagnostic()
                    .wrap_err("failed to flush stdout")?;

                Ok(())
            }
        }
    }
}

struct WorkerBuilder {
    config: ConfigManager,
    client_set: ClientSet,
    envs: Arc<Vec<(String, String)>>,
}

impl runner::WorkerBuilder for WorkerBuilder {
    type Worker = ShellWorker<HttpWorkerClient>;

    fn build(&self, job: JobAcquiredResponse) -> Result<Self::Worker> {
        Ok(ShellWorker {
            root_workdir: self.config.get()?.workdir.clone(),
            envs: Arc::clone(&self.envs),
            client: self.client_set.worker_client(&job.token),
            job,
        })
    }
}

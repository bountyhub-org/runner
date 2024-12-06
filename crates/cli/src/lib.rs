use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use client::hub::{HubClient, RegistrationRequest};
use client::runner::RunnerClient;
use client::worker::WorkerClient;
use client::ClientBuilder;
use ctx::{Background, Ctx};
use error_stack::{Context, Report, Result, ResultExt};
use runner::Runner;
use std::{fmt, io};
use sudoservice::service::Service;
use sudoservice::systemd::{Config as SystemdConfig, Systemd};
use worker::shell::ShellWorker;

pub(crate) mod prompt;

use config::{self, Config};

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
    pub fn run<B, H, I, W>(
        self,
        ctx: Ctx<Background>,
        client_builder: B,
    ) -> Result<(), ApplicationError>
    where
        B: ClientBuilder<Hub = H, Invoker = I, Worker = W> + 'static,
        H: HubClient + 'static,
        I: RunnerClient + 'static,
        W: WorkerClient + 'static,
    {
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
                            prompt::runner_name()
                                .change_context(ApplicationError)
                                .attach_printable("failed to prompt for runner name")?
                        }
                    }
                };

                config::validate_name(&name)
                    .change_context(ApplicationError)
                    .attach_printable("Invalid name")?;

                config::validate_url(&url)
                    .change_context(ApplicationError)
                    .attach_printable("Invalid URL")?;

                config::validate_token(&token)
                    .change_context(ApplicationError)
                    .attach_printable("Invalid token")?;

                config::validate_workdir(&workdir)
                    .change_context(ApplicationError)
                    .attach_printable("Invalid workdir")?;

                config::validate_capacity(capacity)
                    .change_context(ApplicationError)
                    .attach_printable("Invalid capacity")?;

                let request = RegistrationRequest {
                    name,
                    token,
                    workdir,
                };

                let client = client_builder.hub().with_url(url);

                let response = client
                    .register(ctx, &request)
                    .change_context(ApplicationError)
                    .attach_printable("Failed to register runner")?;

                let config = Config {
                    token: response.token,
                    invoker_url: response.invoker_url,
                    fluxy_url: response.fluxy_url,
                    name: request.name,
                    workdir: request.workdir,
                    capacity,
                };

                config
                    .validate()
                    .change_context(ApplicationError)
                    .attach_printable("Failed to validate configuration after registration")?;

                config
                    .write_config()
                    .change_context(ApplicationError)
                    .attach_printable("failed to write config")?;

                tracing::info!("Configuration saved to {}", config::CONFIG_FILE);

                Ok(())
            }
            Commands::Service { action } => {
                let config = Config::read_config()
                    .change_context(ApplicationError)
                    .attach_printable(
                        "Failed to read config file. Please make sure the runner is registered",
                    )?;

                config
                    .validate()
                    .change_context(ApplicationError)
                    .attach_printable("Invalid configuration. Please re-register the runner.")?;

                let svc_name = format!("org.bountyhub.runner.{}", config.name);

                if !Systemd::is_available() {
                    return Err(Report::new(ApplicationError)
                        .attach_printable("Systemd is not available on this system"));
                }

                let worker_envs: Vec<(String, String)> = dotenv::vars().collect();

                let cfg = SystemdConfig {
                    name: svc_name.clone(),
                    display_name: "BountyHub Runner".to_string(),
                    description: Some("BountyHub Runner started as a service".to_string()),
                    username: None,
                    executable: std::env::current_exe()
                        .change_context(ApplicationError)
                        .attach_printable("Failed to get current executable path")?,
                    args: Some(vec!["run".to_string()]),
                    working_directory: Some(
                        std::env::current_dir()
                            .change_context(ApplicationError)
                            .attach_printable("Failed to get current working directory")?,
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
                            .change_context(ApplicationError)
                            .attach_printable("Failed to install service.")?;

                        tracing::info!("Service '{}' installed successfully", svc_name);
                    }
                    ServiceCommands::Start {} => {
                        tracing::debug!("Starting service: {}", svc_name);
                        manager
                            .start()
                            .change_context(ApplicationError)
                            .attach_printable(
                            "Failed to start service. Please make sure the service is installed",
                        )?;
                        tracing::info!("Service '{}' started successfully", svc_name);
                    }
                    ServiceCommands::Stop {} => {
                        tracing::debug!("Stopping service: {}", svc_name);
                        manager
                            .stop()
                            .change_context(ApplicationError)
                            .attach_printable(
                                "Failed to stop service. Please make sure the service is installed",
                            )?;
                        tracing::info!("Service '{}' stopped successfully", svc_name);
                    }
                    ServiceCommands::Uninstall {} => {
                        tracing::debug!("Uninstalling service: {}", svc_name);
                        manager
                            .uninstall()
                            .change_context(ApplicationError)
                            .attach_printable(
                                "Failed to uninstall service. Please make sure the service is installed",
                            )?;
                        tracing::info!("Service '{}' uninstalled successfully", svc_name);
                    }
                    ServiceCommands::Restart {} => {
                        tracing::debug!("Restarting service: {}", svc_name);
                        manager
                            .restart()
                            .change_context(ApplicationError)
                            .attach_printable(
                            "Failed to restart service. Please make sure the service is installed",
                        )?;
                        tracing::info!("Service '{}' restarted successfully", svc_name);
                    }
                    ServiceCommands::Status {} => {
                        tracing::debug!("Checking service status: {}", svc_name);
                        let status = manager
                            .status()
                            .change_context(ApplicationError)
                            .attach_printable(
                            "Failed to get service status. Please make sure the service is installed",
                        )?;
                        tracing::info!("Service '{}' status: {:?}", svc_name, status);
                    }
                }
                Ok(())
            }
            Commands::Run {} => {
                let config = Config::read_config()
                    .change_context(ApplicationError)
                    .attach_printable(
                        "Failed to read config file. Please make sure the runner is registered",
                    )?;

                config
                    .validate()
                    .change_context(ApplicationError)
                    .attach_printable("Invalid configuration. Please re-register the runner.")?;

                let worker_envs: Vec<(String, String)> = dotenv::vars().collect();

                let worker = ShellWorker {
                    root_workdir: config.workdir.clone(),
                    envs: worker_envs,
                    worker_client: client_builder
                        .worker()
                        .with_invoker_url(config.invoker_url.clone())
                        .with_fluxy_url(config.fluxy_url.clone()),
                };

                let runner_client = client_builder
                    .invoker()
                    .with_url(config.invoker_url.clone())
                    .with_token(config.token.clone());

                let mut runner = Runner::new(config, runner_client, worker);

                runner
                    .run(ctx.clone())
                    .change_context(ApplicationError)
                    .attach_printable("runner exited with error")?;

                Ok(())
            }
            Commands::Completion { shell } => {
                let mut cmd = Self::command();
                let name = cmd.get_name().to_string();
                generate(shell, &mut cmd, name, &mut io::stdout());

                Ok(())
            }
        }
    }
}

#[derive(Debug)]
pub struct ApplicationError;

impl fmt::Display for ApplicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Application error")
    }
}

impl Context for ApplicationError {}

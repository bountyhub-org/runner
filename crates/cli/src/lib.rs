use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};
use client::job::HttpJobClient;
use client::pool::ClientPool;
use client::registration::{HttpRegistrationClient, RegistrationClient, RegistrationRequest};
use client::runner::{HttpRunnerClient, JobAcquiredResponse};
use ctx::{Background, Ctx};
use miette::{bail, IntoDiagnostic, Result, WrapErr};
use runner::Runner;
use std::io::{self, Write};
use std::sync::{Arc, RwLock};
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
    pub fn run(self, ctx: Ctx<Background>) -> Result<()> {
        let pool = ClientPool::default();
        let user_agent = Arc::new(format!("runner/{} (cli)", env!("CARGO_PKG_VERSION")));
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

                config::validate_workdir(&workdir).wrap_err("Invalid workdir")?;

                config::validate_capacity(capacity).wrap_err("Invalid capacity")?;

                let request = RegistrationRequest {
                    name,
                    token,
                    workdir,
                };

                let client = HttpRegistrationClient::new(pool.default_client(), &url, &user_agent);

                let response = client
                    .register(ctx, &request)
                    .wrap_err("Failed to register runner")?;

                let config = Config {
                    token: response.token,
                    hub_url: url,
                    invoker_url: response.invoker_url,
                    fluxy_url: response.fluxy_url,
                    name: request.name,
                    workdir: request.workdir,
                    capacity,
                };

                config
                    .validate()
                    .wrap_err("Failed to validate configuration after registration")?;

                config.write().wrap_err("failed to write config")?;

                tracing::info!("Configuration saved to {}", config::CONFIG_FILE);

                Ok(())
            }
            Commands::Service { action } => {
                let config = Config::read().wrap_err(
                    "Failed to read config file. Please make sure the runner is registered",
                )?;

                config
                    .validate()
                    .wrap_err("Invalid configuration. Please re-register the runner.")?;

                let svc_name = format!("org.bountyhub.runner.{}", config.name);

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
                let config = Config::read().wrap_err(
                    "Failed to read config file. Please make sure the runner is registered",
                )?;

                config
                    .validate()
                    .wrap_err("Invalid configuration. Please re-register the runner.")?;

                let config = Arc::new(RwLock::new(config));

                let worker_envs: Arc<Vec<(String, String)>> = Arc::new(dotenv::vars().collect());

                let worker_builder = WorkerBuilder {
                    config: Arc::clone(&config),
                    pool: pool.clone(),
                    user_agent: Arc::clone(&user_agent),
                    envs: Arc::clone(&worker_envs),
                };

                let runner_client = HttpRunnerClient::new(
                    Arc::clone(&config),
                    pool.clone(),
                    Arc::clone(&user_agent),
                );
                let runner = Runner::new(Arc::clone(&config), worker_builder);

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
    config: Arc<RwLock<Config>>,
    pool: ClientPool,
    user_agent: Arc<String>,
    envs: Arc<Vec<(String, String)>>,
}

impl runner::WorkerBuilder for WorkerBuilder {
    type Worker = ShellWorker<HttpJobClient>;

    fn build(&self, job: JobAcquiredResponse) -> Self::Worker {
        ShellWorker {
            root_workdir: self.config.read().unwrap().workdir.clone(),
            envs: Arc::clone(&self.envs),
            client: HttpJobClient::new(
                Arc::clone(&self.config),
                self.pool.clone(),
                &job.token,
                Arc::clone(&self.user_agent),
            ),
            job,
        }
    }
}

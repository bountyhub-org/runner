use clap::Parser;
use cli::Cli;
use miette::Result;
use std::process::exit;
use tokio_util::sync::CancellationToken;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

#[tokio::main]
async fn main() -> Result<()> {
    let token = CancellationToken::new();

    let log_level = match std::env::var("RUNNER_LOG_LEVEL")
        .unwrap_or_else(|_| "INFO".to_string())
        .to_uppercase()
        .as_str()
    {
        "TRACE" => Level::TRACE,
        "DEBUG" => Level::DEBUG,
        "INFO" => Level::INFO,
        "WARN" => Level::WARN,
        "ERROR" => Level::ERROR,
        _ => {
            eprintln!("Invalid log level, defaulting to INFO");
            Level::INFO
        }
    };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_line_number(true)
        .with_file(true)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let background = ctx::background();
    let ctx = background.with_cancel();

    let app = Cli::parse();

    let run_token = token.clone();
    let run_handle = tokio::spawn(async move { app.run(run_ctx).await });

    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for ctrl+c event");
    token.cancel();
    run_handle.await
}

use clap::Parser;
use cli::Cli;
use std::process::exit;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

fn main() {
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

    let run_ctx = ctx.to_background();
    ctrlc::set_handler(move || {
        tracing::info!("Stopping the runner");
        ctx.cancel();
    })
    .expect("Error setting Ctrl-C handler");

    if let Err(err) = app.run(run_ctx) {
        tracing::error!("Error: {:?}", err);
        exit(1);
    }
}

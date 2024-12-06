use clap::Parser;
use cli::Cli;
use client::{HttpClientBuilder, HttpClientBuilderConfig};
use std::process::exit;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

const BANNER: &str = r#"
 ____
|  _ \ _   _ _ __  _ __   ___ _ __
| |_) | | | | '_ \| '_ \ / _ \ '__|
|  _ <| |_| | | | | | | |  __/ |
|_| \_\\__,_|_| |_|_| |_|\___|_|

"#;

fn main() {
    println!("{}", BANNER);

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

    let client_builder = HttpClientBuilder::new(HttpClientBuilderConfig {
        user_agent: format!("runner/{} (CLI)", env!("CARGO_PKG_VERSION")),
    });

    if let Err(err) = app.run(run_ctx, client_builder) {
        tracing::error!("Error: {:?}", err);
        exit(1);
    }
}
use ctx::{Background, Ctx};
use miette::{IntoDiagnostic, Result, WrapErr};
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    str::FromStr,
    sync::mpsc,
    thread,
    time::Duration,
};

pub(crate) fn runner_name(ctx: &Ctx<Background>, default: &str) -> Result<String> {
    let mut stdout = io::stdout();

    let default_runner_name = default.to_string();

    let mut buf = String::new();
    loop {
        if ctx.is_done() {
            return Err(miette::miette!("Operation cancelled"));
        }

        buf.clear();
        stdout
            .write_all(format!("Runner name({}): ", default_runner_name).as_bytes())
            .into_diagnostic()
            .wrap_err("Failed to write to stdout")?;

        stdout
            .flush()
            .into_diagnostic()
            .wrap_err("Failed to flush")?;

        // Use a channel to make read_line interruptible
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut local_buf = String::new();
            match io::stdin().lock().read_line(&mut local_buf) {
                Ok(_) => {
                    let _ = tx.send(Ok(local_buf));
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
        });

        // Wait for input with timeout, checking context
        loop {
            if ctx.is_done() {
                return Err(miette::miette!("Operation cancelled"));
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(line)) => {
                    buf = line;
                    break;
                }
                Ok(Err(e)) => {
                    return Err(e)
                        .into_diagnostic()
                        .wrap_err("Failed to read from stdin");
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Continue checking context
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(miette::miette!("Input thread disconnected"));
                }
            }
        }

        let name = buf.trim().to_string();
        if name.is_empty() {
            return Ok(default_runner_name);
        }
        match config::validate_name(&name) {
            Ok(_) => return Ok(name),
            Err(e) => {
                stdout
                    .write_all(format!("Invalid runner name: {}\n", e).as_bytes())
                    .into_diagnostic()
                    .wrap_err("Failed to write to stdout")?;
            }
        }
    }
}

pub(crate) fn runner_workdir(ctx: &Ctx<Background>, default: PathBuf) -> Result<PathBuf> {
    let mut stdout = io::stdout();

    let mut buf = String::new();
    loop {
        if ctx.is_done() {
            return Err(miette::miette!("Operation cancelled"));
        }

        buf.clear();
        stdout
            .write_all(format!("Runner workdir({default:?}): ").as_bytes())
            .into_diagnostic()
            .wrap_err("Failed to write to stdout")?;

        stdout
            .flush()
            .into_diagnostic()
            .wrap_err("Failed to flush")?;

        // Use a channel to make read_line interruptible
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut local_buf = String::new();
            match io::stdin().lock().read_line(&mut local_buf) {
                Ok(_) => {
                    let _ = tx.send(Ok(local_buf));
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
        });

        // Wait for input with timeout, checking context
        loop {
            if ctx.is_done() {
                return Err(miette::miette!("Operation cancelled"));
            }
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(line)) => {
                    buf = line;
                    break;
                }
                Ok(Err(e)) => {
                    return Err(e)
                        .into_diagnostic()
                        .wrap_err("Failed to read from stdin");
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Continue checking context
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(miette::miette!("Input thread disconnected"));
                }
            }
        }

        let workdir = buf.trim().to_string();
        if workdir.is_empty() {
            return Ok(default);
        }

        let workdir = PathBuf::from_str(&workdir)
            .into_diagnostic()
            .wrap_err("Failed to parse workdir path")?;

        match config::validate_workdir(&workdir) {
            Ok(_) => return Ok(workdir),
            Err(e) => {
                stdout
                    .write_all(format!("Invalid runner workdir: {}\n", e).as_bytes())
                    .into_diagnostic()
                    .wrap_err("Failed to write to stdout")?;
            }
        }
    }
}

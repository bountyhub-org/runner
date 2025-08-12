use miette::{IntoDiagnostic, Result, WrapErr};
use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    str::FromStr,
};

pub(crate) fn runner_name(default: &str) -> Result<String> {
    let mut stdout = io::stdout();
    let stdin = io::stdin();

    let default_runner_name = default.to_string();

    let mut buf = String::new();
    loop {
        buf.clear();
        stdout
            .write_all(format!("Runner name({}): ", default_runner_name).as_bytes())
            .into_diagnostic()
            .wrap_err("Failed to write to stdout")?;

        stdout
            .flush()
            .into_diagnostic()
            .wrap_err("Failed to flush")?;

        stdin
            .lock()
            .read_line(&mut buf)
            .into_diagnostic()
            .wrap_err("Failed to read from stdin")?;

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

pub(crate) fn runner_workdir(default: PathBuf) -> Result<PathBuf> {
    let mut stdout = io::stdout();
    let stdin = io::stdin();

    let mut buf = String::new();
    loop {
        buf.clear();
        stdout
            .write_all(format!("Runner workdir({default:?}): ").as_bytes())
            .into_diagnostic()
            .wrap_err("Failed to write to stdout")?;

        stdout
            .flush()
            .into_diagnostic()
            .wrap_err("Failed to flush")?;

        stdin
            .lock()
            .read_line(&mut buf)
            .into_diagnostic()
            .wrap_err("Failed to read from stdin")?;

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
                    .write_all(format!("Invalid runner name: {}\n", e).as_bytes())
                    .into_diagnostic()
                    .wrap_err("Failed to write to stdout")?;
            }
        }
    }
}

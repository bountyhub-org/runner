use miette::{IntoDiagnostic, Result, WrapErr};
use tokio::io::{stdin, stdout, AsyncBufReadExt, AsyncWriteExt, BufReader};

pub(crate) async fn runner_name() -> Result<String> {
    let mut stdout = stdout();
    let mut stdin = BufReader::new(stdin());

    let default_runner_name = config::generate_default_name();

    let mut buf = String::new();
    loop {
        buf.clear();
        stdout
            .write_all(format!("Runner name({}): ", default_runner_name).as_bytes())
            .await
            .into_diagnostic()
            .wrap_err("Failed to write to stdout")?;

        stdout
            .flush()
            .await
            .into_diagnostic()
            .wrap_err("Failed to flush stdout")?;

        stdin
            .read_line(&mut buf)
            .await
            .into_diagnostic()
            .wrap_err("Failed to read stdin")?;

        let name = buf.trim().to_string();
        if name.is_empty() {
            return Ok(default_runner_name);
        }
        match config::validate_name(&name) {
            Ok(_) => return Ok(name),
            Err(e) => {
                stdout
                    .write_all(format!("Invalid runner name: {}\n", e).as_bytes())
                    .await
                    .into_diagnostic()
                    .wrap_err("Failed to write to stdout")?;
            }
        }
    }
}

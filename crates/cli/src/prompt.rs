use error_stack::{Context, Result, ResultExt};
use std::fmt;
use std::io::{self, BufRead, Write};

pub(crate) fn runner_name() -> Result<String, PromptError> {
    let mut stdout = io::stdout();
    let stdin = io::stdin();

    let default_runner_name = config::generate_default_name();

    let mut buf = String::new();
    loop {
        buf.clear();
        stdout
            .write_all(format!("Runner name({}): ", default_runner_name).as_bytes())
            .change_context(PromptError)
            .attach_printable("Failed to write to stdout")?;

        stdout
            .flush()
            .change_context(PromptError)
            .attach_printable("Failed to flush")?;

        stdin
            .lock()
            .read_line(&mut buf)
            .change_context(PromptError)
            .attach_printable("Failed to read from stdin")?;

        let name = buf.trim().to_string();
        if name.is_empty() {
            return Ok(default_runner_name);
        }
        match config::validate_name(&name) {
            Ok(_) => return Ok(name),
            Err(e) => {
                stdout
                    .write_all(format!("Invalid runner name: {}\n", e).as_bytes())
                    .change_context(PromptError)
                    .attach_printable("Failed to write to stdout")?;
            }
        }
    }
}

#[derive(Debug)]
pub struct PromptError;

impl fmt::Display for PromptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Prompt error")
    }
}

impl Context for PromptError {}

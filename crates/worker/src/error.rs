use error_stack::Context;
use std::fmt;

#[derive(Debug)]
pub struct WorkerError;

impl fmt::Display for WorkerError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Worker error")
    }
}

impl Context for WorkerError {}

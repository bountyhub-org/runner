use error_stack::Context;
use std::fmt;

#[derive(Debug)]
pub struct RetryableError;

impl fmt::Display for RetryableError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "RetryableError")
    }
}

impl Context for RetryableError {}

#[derive(Debug)]
pub struct FatalError;

impl fmt::Display for FatalError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "FatalError")
    }
}

impl Context for FatalError {}

#[derive(Debug)]
pub struct ClientError;

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ClientError")
    }
}

impl Context for ClientError {}

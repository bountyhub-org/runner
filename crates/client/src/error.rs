use miette::Diagnostic;
use std::io;
use thiserror::Error;
use ureq::Error as UreqError;

#[derive(Diagnostic, Debug, Error)]
#[diagnostic(code(client::client_error))]
pub enum ClientError {
    #[error("Retryable error")]
    RetryableError,

    // TODO: handle 409
    #[error("Fatal error")]
    FatalError,
}

impl From<UreqError> for ClientError {
    fn from(value: UreqError) -> Self {
        match value {
            UreqError::Status(500.., ..) => ClientError::RetryableError,
            UreqError::Status(..) => ClientError::FatalError,
            UreqError::Transport(err) => match err.kind() {
                ureq::ErrorKind::ProxyConnect | ureq::ErrorKind::ConnectionFailed => {
                    ClientError::RetryableError
                }
                _e => ClientError::FatalError,
            },
        }
    }
}

#[derive(Diagnostic, Debug, Error)]
#[diagnostic(code(client::operation_error))]
pub enum OperationError {
    #[error("io error")]
    IoError(#[from] io::Error),
    #[error("max retries reached")]
    MaxRetriesError,
}

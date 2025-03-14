use miette::Diagnostic;
use std::io;
use thiserror::Error;
use ureq::{Error as UreqError, Transport};

#[derive(Diagnostic, Debug, Error)]
#[diagnostic(code(client::client_error))]
pub enum ClientError {
    #[error("Server error")]
    ServerError,

    #[error("Unauthorized")]
    UnauthorizedError,

    #[error("Conflict")]
    ConflictError,

    #[error("Response error")]
    ResponseError(u16),

    #[error("Connection error")]
    ConnectionError(Transport),

    #[error("Fatal error")]
    FatalError(Transport),

    #[error("Operation cancelled")]
    CancellationError,
}

impl From<UreqError> for ClientError {
    fn from(value: UreqError) -> Self {
        match value {
            UreqError::Status(500.., ..) => ClientError::ServerError,
            UreqError::Status(401 | 403, ..) => ClientError::UnauthorizedError,
            UreqError::Status(409, ..) => ClientError::ConflictError,
            UreqError::Status(status, ..) => ClientError::ResponseError(status),
            UreqError::Transport(e) => match e.kind() {
                ureq::ErrorKind::ProxyConnect | ureq::ErrorKind::ConnectionFailed => {
                    ClientError::ConnectionError(e)
                }
                _e => ClientError::FatalError(e),
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

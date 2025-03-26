use miette::Diagnostic;
use std::io;
use thiserror::Error;
use ureq::{Error as UreqError, Timeout};

#[derive(Diagnostic, Debug, Error)]
#[diagnostic(code(client::client_error))]
pub enum ClientError {
    #[error("Server error: {0}")]
    ServerError(u16),

    #[error("Unauthorized")]
    UnauthorizedError,

    #[error("Conflict")]
    ConflictError,

    #[error("Response error")]
    ResponseError(u16),

    #[error("Connection error: {0:?}")]
    ConnectionError(ConnectionError),

    #[error("Client error: {0:?}")]
    ClientError(UreqError),

    #[error("Operation cancelled")]
    CancellationError,

    #[error("IO error: {0:?}")]
    IoError(#[from] io::Error),
}

impl From<UreqError> for ClientError {
    fn from(value: UreqError) -> Self {
        match value {
            UreqError::StatusCode(code @ 500..) => ClientError::ServerError(code),
            UreqError::StatusCode(401 | 403) => ClientError::UnauthorizedError,
            UreqError::StatusCode(409) => ClientError::ConflictError,
            UreqError::StatusCode(code) => ClientError::ResponseError(code),
            UreqError::Timeout(timeout) => {
                ClientError::ConnectionError(ConnectionError::TimeoutError(timeout))
            }
            err => ClientError::ClientError(err),
        }
    }
}

impl From<recoil::Error<ClientError>> for ClientError {
    fn from(value: recoil::Error<ClientError>) -> Self {
        match value {
            recoil::Error::MaxRetriesReachedError => {
                ClientError::ConnectionError(ConnectionError::MaxRetriesError)
            }
            recoil::Error::UserError(e) => e,
        }
    }
}

impl ClientError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ClientError::ConnectionError(_) | ClientError::ServerError(_)
        )
    }
}

#[derive(Diagnostic, Debug, Error)]
#[diagnostic(code(client::operation_error))]
pub enum ConnectionError {
    #[error("Max retries reached")]
    MaxRetriesError,

    #[error("Timeout error")]
    TimeoutError(Timeout),
}

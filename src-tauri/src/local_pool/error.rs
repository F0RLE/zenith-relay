use serde::Serialize;
use std::fmt::{self, Display};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Io,
    Conflict,
    GatewayUnavailable,
    InvalidState,
    NotFound,
    ProfileRestoreBlocked,
    RecoveryRequired,
    SecretStoreUnavailable,
    UnsupportedSchema,
}

#[derive(Clone, Debug)]
pub struct LocalPoolError {
    pub code: ErrorCode,
    pub message: String,
}

impl LocalPoolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl Display for LocalPoolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for LocalPoolError {}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: ErrorCode,
    pub message: String,
}

impl From<LocalPoolError> for CommandError {
    fn from(error: LocalPoolError) -> Self {
        Self {
            code: error.code,
            message: error.message,
        }
    }
}

pub type Result<T> = std::result::Result<T, LocalPoolError>;

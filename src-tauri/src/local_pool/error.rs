use serde::Serialize;
use std::fmt::{self, Display};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Io,
    Conflict,
    GatewayUnavailable,
    SourceTestFailed,
    InvalidState,
    NotFound,
    ProfileRestoreBlocked,
    RecoveryRequired,
    SecretStoreUnavailable,
    UnsupportedSchema,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorDiagnostics {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct LocalPoolError {
    pub code: ErrorCode,
    pub message: String,
    pub diagnostic: Option<Box<ErrorDiagnostics>>,
}

impl LocalPoolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            diagnostic: None,
        }
    }

    pub fn with_diagnostic(mut self, diagnostic: ErrorDiagnostics) -> Self {
        self.diagnostic = Some(Box::new(diagnostic));
        self
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<Box<ErrorDiagnostics>>,
}

impl From<LocalPoolError> for CommandError {
    fn from(error: LocalPoolError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            diagnostic: error.diagnostic,
        }
    }
}

pub type Result<T> = std::result::Result<T, LocalPoolError>;

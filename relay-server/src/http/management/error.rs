use axum::response::{IntoResponse, Response};
use axum::{http::StatusCode, Json};
use zenith_relay_core::protocol::{ApiError, ErrorEnvelope};

#[derive(Debug)]
pub struct ManagementError {
    pub(super) status: StatusCode,
    pub(super) code: String,
    pub(super) message: String,
    pub(super) stage: String,
    pub(super) retryable: bool,
}

impl ManagementError {
    pub(super) fn validation(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message, "validation", false)
    }

    pub(super) fn not_found(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message, "lookup", false)
    }

    pub(super) fn conflict(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, code, message, "policy", false)
    }

    pub(super) fn internal(code: &str, message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            code,
            message,
            "server",
            true,
        )
    }

    pub(super) fn new(
        status: StatusCode,
        code: &str,
        message: impl Into<String>,
        stage: &str,
        retryable: bool,
    ) -> Self {
        Self {
            status,
            code: code.to_string(),
            message: message.into(),
            stage: stage.to_string(),
            retryable,
        }
    }
}

impl IntoResponse for ManagementError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorEnvelope {
                error: ApiError {
                    code: self.code,
                    message: self.message,
                    stage: self.stage,
                    retryable: self.retryable,
                    request_id: uuid::Uuid::new_v4().to_string(),
                },
            }),
        )
            .into_response()
    }
}

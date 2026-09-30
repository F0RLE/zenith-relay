use crate::error::{normalize_error_code, safe_error_code};
use crate::error_codes;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaRefreshFailure {
    pub code: String,
    pub retryable: bool,
    http_status: Option<u16>,
    retry_after_ms: Option<u64>,
}

impl QuotaRefreshFailure {
    pub fn new(code: &str, retryable: bool) -> Self {
        Self {
            code: safe_error_code(code),
            retryable,
            http_status: None,
            retry_after_ms: None,
        }
    }

    pub fn http_status(&self) -> Option<u16> {
        self.http_status
    }

    pub fn retry_after_ms(&self) -> Option<u64> {
        self.retry_after_ms
    }

    pub(crate) fn with_retry_after(mut self, delay: Option<u64>) -> Self {
        self.retry_after_ms = delay;
        self
    }

    pub(crate) fn with_http_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }
}

pub fn classify_quota_http_failure(status: u16, body: &[u8]) -> QuotaRefreshFailure {
    let retryable = status == 429 || status >= 500;
    let code = provider_error_code(body).unwrap_or_else(|| {
        match status {
            401 => error_codes::QUOTA_UNAUTHORIZED,
            403 => error_codes::QUOTA_FORBIDDEN,
            429 => error_codes::QUOTA_RATE_LIMITED,
            500..=599 => error_codes::QUOTA_UPSTREAM,
            _ => error_codes::QUOTA_HTTP_STATUS,
        }
        .to_string()
    });
    QuotaRefreshFailure::new(&code, retryable).with_http_status(status)
}

fn provider_error_code(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    [
        "/detail/code",
        "/detail/error/code",
        "/error/code",
        "/code",
        "/error/type",
        "/type",
    ]
    .into_iter()
    .filter_map(|pointer| value.pointer(pointer).and_then(serde_json::Value::as_str))
    .find_map(normalize_error_code)
}

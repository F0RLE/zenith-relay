use crate::error::{normalize_error_code, safe_error_code};
use crate::error_codes;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum QuotaTransportFailure {
    StaleScope,
    InvalidOrigin,
    AdmissionFull,
    AdmissionTimeout,
    Timeout,
    Network,
    BodyTimeout,
    BodyTransport,
}

impl QuotaTransportFailure {
    fn as_str(self) -> &'static str {
        match self {
            Self::StaleScope => "stale_scope",
            Self::InvalidOrigin => "invalid_origin",
            Self::AdmissionFull => "admission_full",
            Self::AdmissionTimeout => "admission_timeout",
            Self::Timeout => "timeout",
            Self::Network => "network",
            Self::BodyTimeout => "body_timeout",
            Self::BodyTransport => "body_transport",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaRefreshFailure {
    pub code: String,
    pub retryable: bool,
    http_status: Option<u16>,
    retry_after_ms: Option<u64>,
    endpoint_stage: Option<&'static str>,
    response_format: Option<&'static str>,
    response_mitigation: Option<&'static str>,
    transport_failure: Option<QuotaTransportFailure>,
    cf_ray: Option<Box<str>>,
}

impl QuotaRefreshFailure {
    pub fn new(code: &str, retryable: bool) -> Self {
        Self {
            code: safe_error_code(code),
            retryable,
            http_status: None,
            retry_after_ms: None,
            endpoint_stage: None,
            response_format: None,
            response_mitigation: None,
            transport_failure: None,
            cf_ray: None,
        }
    }

    pub fn http_status(&self) -> Option<u16> {
        self.http_status
    }

    pub fn retry_after_ms(&self) -> Option<u64> {
        self.retry_after_ms
    }

    pub fn endpoint_stage(&self) -> Option<&'static str> {
        self.endpoint_stage
    }

    pub fn response_format(&self) -> Option<&'static str> {
        self.response_format
    }

    pub fn response_mitigation(&self) -> Option<&'static str> {
        self.response_mitigation
    }

    pub fn transport_failure(&self) -> Option<&'static str> {
        self.transport_failure.map(QuotaTransportFailure::as_str)
    }

    pub fn cf_ray(&self) -> Option<&str> {
        self.cf_ray.as_deref()
    }

    pub(crate) fn with_cf_ray(mut self, value: Option<&str>) -> Self {
        self.cf_ray = Self::validated_cf_ray(value);
        self
    }

    pub(crate) fn validated_cf_ray(value: Option<&str>) -> Option<Box<str>> {
        value
            .filter(|value| {
                let Some((id, location)) = value.split_once('-') else {
                    return false;
                };
                id.len() == 16
                    && id.bytes().all(|byte| byte.is_ascii_hexdigit())
                    && location.len() == 3
                    && location.bytes().all(|byte| byte.is_ascii_uppercase())
            })
            .map(Into::into)
    }

    pub(crate) fn with_endpoint_stage(mut self, stage: &'static str) -> Self {
        self.endpoint_stage = Some(stage);
        self
    }

    pub(crate) fn with_response_format(mut self, format: &'static str) -> Self {
        self.response_format = Some(format);
        self
    }

    pub(crate) fn with_response_mitigation(mut self, mitigation: Option<&'static str>) -> Self {
        self.response_mitigation = mitigation;
        self
    }

    pub(crate) fn with_transport_failure(mut self, failure: QuotaTransportFailure) -> Self {
        self.transport_failure = Some(failure);
        self
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

pub fn classify_quota_http_failure(status: u16, response_body: &[u8]) -> QuotaRefreshFailure {
    let retryable = status == 429 || status >= 500;
    let code = provider_error_code(response_body).unwrap_or_else(|| {
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

fn provider_error_code(response_body: &[u8]) -> Option<String> {
    let error_payload: serde_json::Value = serde_json::from_slice(response_body).ok()?;
    [
        "/detail/code",
        "/detail/error/code",
        "/error/code",
        "/code",
        "/error/type",
        "/type",
    ]
    .into_iter()
    .filter_map(|pointer| {
        error_payload
            .pointer(pointer)
            .and_then(serde_json::Value::as_str)
    })
    .find_map(normalize_error_code)
}

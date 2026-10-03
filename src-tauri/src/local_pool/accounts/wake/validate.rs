use super::{
    WakeExecutionErrorCode, WakeExecutionFailure, MAX_LOCAL_ACCOUNT_ID_BYTES, MAX_MODEL_ID_BYTES,
    MAX_OUTPUT_TOKEN_CAP,
};
use std::time::Instant;
use url::Url;
use zenith_relay_core::automations::WakeExecutionRequest;
use zenith_relay_core::{is_http_endpoint, is_loopback_url, url_has_userinfo};

pub(super) fn validate_endpoint(endpoint: &Url) -> Result<(), WakeExecutionFailure> {
    if !is_http_endpoint(endpoint)
        || url_has_userinfo(endpoint)
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(WakeExecutionFailure::invalid(
            WakeExecutionErrorCode::InvalidEndpoint,
        ));
    }
    if endpoint.scheme() == "http" && !is_loopback_url(endpoint) {
        return Err(WakeExecutionFailure::invalid(
            WakeExecutionErrorCode::InvalidEndpoint,
        ));
    }
    Ok(())
}

pub(super) fn validate_secret(
    value: &str,
    max_bytes: usize,
    code: WakeExecutionErrorCode,
) -> Result<(), WakeExecutionFailure> {
    if value.is_empty() || value.len() > max_bytes {
        return Err(WakeExecutionFailure::invalid(code));
    }
    Ok(())
}

pub(super) fn validate_request(request: &WakeExecutionRequest) -> Result<(), WakeExecutionFailure> {
    let account_id = request.account_id.trim();
    if !zenith_relay_core::is_ascii_token(account_id, MAX_LOCAL_ACCOUNT_ID_BYTES) {
        return Err(WakeExecutionFailure::invalid(
            WakeExecutionErrorCode::InvalidRequest,
        ));
    }
    let model = request.model_id.trim();
    if model.is_empty()
        || model.len() > MAX_MODEL_ID_BYTES
        || model.bytes().any(|byte| byte.is_ascii_control())
        || !(1..=MAX_OUTPUT_TOKEN_CAP).contains(&request.output_token_cap)
    {
        return Err(WakeExecutionFailure::invalid(
            WakeExecutionErrorCode::InvalidRequest,
        ));
    }
    Ok(())
}

pub(super) fn status_failure(status: u16, latency_ms: u64) -> WakeExecutionFailure {
    let (code, retryable) = match status {
        401 => (WakeExecutionErrorCode::Unauthorized, false),
        403 => (WakeExecutionErrorCode::Forbidden, false),
        429 => (WakeExecutionErrorCode::RateLimited, true),
        500..=599 => (WakeExecutionErrorCode::Upstream, true),
        _ => (WakeExecutionErrorCode::HttpStatus, false),
    };
    WakeExecutionFailure::runtime(code, retryable, Some(status), latency_ms)
}

pub(super) fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

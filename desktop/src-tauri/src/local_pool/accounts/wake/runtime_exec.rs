use super::{WakeExecutionErrorCode, WakeExecutionFailure, WakeExecutionMetrics};
use axum::{body::Body, http::Response};
use std::sync::Arc;
use std::time::Instant;
use zenith_relay_core::GatewayRuntime;
use zenith_relay_core::automations::WakeExecutionRequest;
use zenith_relay_core::error_codes;
use zenith_relay_core::gateway::{AccountWakeRequest, execute_account_wake};

pub(super) const MAX_RESPONSE_BYTES: usize = 256 * 1024;

/// The runtime owns authentication, transport, scheduler leases and usage.
/// This adapter keeps only bounded metrics from its sanitized response.
pub async fn execute_with_runtime(
    runtime: Arc<GatewayRuntime>,
    local_key_id: &str,
    request: &WakeExecutionRequest,
) -> Result<WakeExecutionMetrics, WakeExecutionFailure> {
    let started = Instant::now();
    let wake_response = execute_account_wake(
        runtime,
        AccountWakeRequest {
            local_key_id: local_key_id.to_string(),
            account_id: request.account_id.clone(),
            model_id: request.model_id.clone(),
            output_token_cap: request.output_token_cap,
        },
    )
    .await;
    metrics_from_response(wake_response, started).await
}

pub(super) async fn metrics_from_response(
    wake_response: Response<Body>,
    started: Instant,
) -> Result<WakeExecutionMetrics, WakeExecutionFailure> {
    let status = wake_response.status();
    let category = wake_response
        .headers()
        .get("x-zenith-relay-error-category")
        .and_then(|header_value| header_value.to_str().ok())
        .map(str::to_string);
    if !status.is_success() {
        return Err(runtime_status_failure(
            status,
            category.as_deref(),
            elapsed_ms(started),
        ));
    }
    let response_body = axum::body::to_bytes(wake_response.into_body(), MAX_RESPONSE_BYTES)
        .await
        .map_err(|_| {
            WakeExecutionFailure::runtime(
                WakeExecutionErrorCode::ResponseTooLarge,
                false,
                Some(status.as_u16()),
                elapsed_ms(started),
            )
        })?;
    let response_json: serde_json::Value =
        serde_json::from_slice(&response_body).map_err(|_| {
            WakeExecutionFailure::runtime(
                WakeExecutionErrorCode::InvalidResponse,
                false,
                Some(status.as_u16()),
                elapsed_ms(started),
            )
        })?;
    let usage = wake_usage(&response_json);
    Ok(WakeExecutionMetrics {
        http_status: status.as_u16(),
        latency_ms: elapsed_ms(started),
        input_tokens: usage
            .and_then(|usage| usage_token(usage, &["input_tokens", "prompt_tokens"])),
        output_tokens: usage
            .and_then(|usage| usage_token(usage, &["output_tokens", "completion_tokens"])),
        total_tokens: usage.and_then(|usage| usage_token(usage, &["total_tokens"])),
    })
}

fn runtime_status_failure(
    status: reqwest::StatusCode,
    category: Option<&str>,
    latency_ms: u64,
) -> WakeExecutionFailure {
    let (code, retryable) = match category {
        Some(
            error_codes::ALL_SOURCES_COOLING_DOWN
            | error_codes::ALL_SOURCES_TEMPORARILY_UNAVAILABLE
            | error_codes::NO_ELIGIBLE_SOURCE,
        ) => (WakeExecutionErrorCode::Upstream, true),
        Some(
            error_codes::MODEL_NOT_FOUND
            | error_codes::UPSTREAM_MODEL_NOT_FOUND
            | error_codes::UPSTREAM_MODEL_UNSUPPORTED,
        ) => (WakeExecutionErrorCode::ModelUnavailable, false),
        Some(error_codes::INVALID_REQUEST) => (WakeExecutionErrorCode::InvalidRequest, false),
        Some(
            error_codes::REQUEST_TIMEOUT
            | error_codes::UPSTREAM_REQUEST_TIMEOUT
            | error_codes::UPSTREAM_GATEWAY_TIMEOUT
            | error_codes::UPSTREAM_TRANSPORT_TIMEOUT,
        ) => (WakeExecutionErrorCode::Timeout, true),
        Some(
            error_codes::UPSTREAM_TRANSPORT
            | error_codes::UPSTREAM_TRANSPORT_CONNECT
            | error_codes::UPSTREAM_TRANSPORT_REQUEST
            | error_codes::UPSTREAM_TRANSPORT_BODY,
        ) => (WakeExecutionErrorCode::Transport, true),
        _ => match status {
            reqwest::StatusCode::UNAUTHORIZED => (WakeExecutionErrorCode::Unauthorized, false),
            reqwest::StatusCode::FORBIDDEN => (WakeExecutionErrorCode::Forbidden, false),
            reqwest::StatusCode::TOO_MANY_REQUESTS => (WakeExecutionErrorCode::RateLimited, true),
            status if status.is_server_error() => (WakeExecutionErrorCode::Upstream, true),
            reqwest::StatusCode::BAD_REQUEST => (WakeExecutionErrorCode::InvalidRequest, false),
            _ => (WakeExecutionErrorCode::HttpStatus, false),
        },
    };
    WakeExecutionFailure::runtime(code, retryable, Some(status.as_u16()), latency_ms)
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn wake_usage(response_payload: &serde_json::Value) -> Option<&serde_json::Value> {
    response_payload
        .get("usage")
        .or_else(|| response_payload.pointer("/response/usage"))
        .or_else(|| response_payload.pointer("/response/response/usage"))
        .or_else(|| response_payload.get("usageMetadata"))
}

fn usage_token(usage_value: &serde_json::Value, token_field_names: &[&str]) -> Option<u64> {
    token_field_names.iter().find_map(|field_name| {
        usage_value
            .get(*field_name)
            .and_then(serde_json::Value::as_u64)
    })
}

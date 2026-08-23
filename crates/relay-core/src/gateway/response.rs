use super::errors::{upstream_failure_status, AttemptFailure, RateLimitBodyHint};
use super::now_ms;
use super::streaming::{parse_sse_event, TerminalOutcome};
use crate::protocol::sse_event_end;
use crate::runtime::{DefaultServiceTier, ExecutorRoute};
use crate::usage::ReasoningEffortDiagnostics;
use crate::{CacheWriteTtl, Error, ErrorOrigin, GatewayRuntime, ToolUseDiagnostics, UsageEvent};
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;

pub(super) type CompletionCallback =
    Arc<dyn Fn(&mut UsageEvent, Option<&str>, RateLimitBodyHint) + Send + Sync>;

pub(super) fn upstream_body_error_response(
    runtime: &GatewayRuntime,
    mut event: UsageEvent,
    started: Instant,
    error: Error,
) -> Response<Body> {
    event.success = false;
    event.http_status = StatusCode::BAD_GATEWAY.as_u16();
    let too_large = matches!(error, Error::UpstreamBodyTooLarge);
    let category = if too_large {
        "upstream_body_too_large"
    } else {
        "upstream_body"
    };
    event.error_category = Some(category.to_string());
    event.latency_ms = started.elapsed().as_millis() as u64;
    let origin = event.error_origin().unwrap_or(ErrorOrigin::Relay);
    let request_id = event.request_id.clone();
    emit_usage(runtime, event);
    super::errors::api_error_with_origin_and_category(
        StatusCode::BAD_GATEWAY,
        if too_large {
            "upstream response is too large"
        } else {
            "upstream response failed"
        },
        "upstream_error",
        category,
        origin,
        Some(&request_id),
    )
}

pub(super) fn proxy_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder().status(status).body(body).unwrap();
    copy_safe_upstream_headers(response.headers_mut(), upstream_headers, true);
    response
}

pub(super) fn proxy_error_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
    origin: ErrorOrigin,
    category: &str,
    request_id: Option<&str>,
) -> Response<Body> {
    let mut response = proxy_response(status, upstream_headers, body);
    attach_error_diagnostics(&mut response, origin, category, request_id);
    response
}

pub(super) fn attach_error_diagnostics(
    response: &mut Response<Body>,
    origin: ErrorOrigin,
    category: &str,
    request_id: Option<&str>,
) {
    response.headers_mut().insert(
        "x-zenith-relay-error-origin",
        HeaderValue::from_static(origin.as_str()),
    );
    if let Ok(value) = HeaderValue::from_str(category) {
        response
            .headers_mut()
            .insert("x-zenith-relay-error-category", value);
    }
    if let Some(request_id) = request_id.and_then(safe_request_id) {
        if let Ok(value) = HeaderValue::from_str(request_id) {
            response
                .headers_mut()
                .insert("x-zenith-relay-request-id", value);
        }
    }
}

pub(super) fn attach_stream_diagnostics(
    response: &mut Response<Body>,
    origin: ErrorOrigin,
    request_id: &str,
) {
    response.headers_mut().insert(
        "x-zenith-relay-upstream-origin",
        HeaderValue::from_static(origin.as_str()),
    );
    if let Some(request_id) = safe_request_id(request_id) {
        if let Ok(value) = HeaderValue::from_str(request_id) {
            response
                .headers_mut()
                .insert("x-zenith-relay-request-id", value);
        }
    }
}

fn safe_request_id(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 128
        && value.is_ascii()
        && !value.chars().any(char::is_control))
    .then_some(value)
}

pub(super) fn route_error_origin(route: &ExecutorRoute) -> ErrorOrigin {
    if route.account_id.is_some() {
        ErrorOrigin::Account
    } else {
        ErrorOrigin::Provider
    }
}

pub(super) fn proxy_sse_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder().status(status).body(body).unwrap();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    copy_safe_upstream_headers(response.headers_mut(), upstream_headers, false);
    response
}

pub(super) fn proxy_json_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder().status(status).body(body).unwrap();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    copy_safe_upstream_headers(response.headers_mut(), upstream_headers, false);
    response
}

/// Copies only response metadata that a native client can safely use for
/// retries and diagnostics. Credentials, cookies, transport headers, and
/// provider server details are never reflected to the local client.
fn copy_safe_upstream_headers(
    target: &mut HeaderMap,
    upstream: &reqwest::header::HeaderMap,
    include_content_type: bool,
) {
    for (name, value) in upstream {
        let name = name.as_str();
        let allowed = (include_content_type && name == CONTENT_TYPE.as_str())
            || matches!(
                name,
                "cache-control"
                    | "retry-after"
                    | "request-id"
                    | "x-request-id"
                    | "x-should-retry"
                    | "openai-processing-ms"
            )
            || name.starts_with("anthropic-ratelimit-")
            || name.starts_with("x-ratelimit-");
        if allowed {
            target.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes())
                    .expect("upstream header name is valid"),
                HeaderValue::from_bytes(value.as_bytes()).expect("upstream header value is valid"),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn usage_event(
    request_id: &str,
    attempt: u16,
    local_key_id: &str,
    route: &ExecutorRoute,
    reasoning_effort: Option<&ReasoningEffortDiagnostics>,
    requested_model: &str,
    success: bool,
    http_status: u16,
    error_category: Option<String>,
    latency_ms: u64,
    tool_use: ToolUseDiagnostics,
) -> UsageEvent {
    let mut event = UsageEvent {
        request_id: request_id.to_string(),
        attempt,
        local_key_id: local_key_id.to_string(),
        source_id: route.source_id.clone(),
        candidate_id: Some(route.candidate_id.clone()),
        account_id: route.account_id.clone(),
        client_context_id: route.client_context_id.clone(),
        routing: route.routing.clone(),
        requested_model: Some(requested_model.to_string()),
        resolved_model: Some(route.source_model.clone()),
        requested_reasoning_effort: None,
        effective_reasoning_effort: None,
        wire_api: route.wire_api,
        service_tier: route.service_tier,
        applied_service_tier: None,
        success,
        http_status,
        error_category,
        tool_use,
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: None,
        latency_ms,
        ttft_ms: None,
        generation_ms: None,
        input_tokens: None,
        cached_input_tokens: None,
        cache_write_input_tokens: None,
        // Usage records describe what the upstream actually reported, not the
        // configured preference. `apply_usage` sets this only for a cache write.
        cache_write_ttl: None,
        reasoning_tokens: None,
        output_tokens: None,
        total_tokens: None,
        quota_snapshot: None,
    };
    if let Some(reasoning_effort) = reasoning_effort {
        reasoning_effort.apply_to(&mut event);
    }
    event
}

pub(super) fn populate_tokens(event: &mut UsageEvent, body: &[u8]) {
    let Ok(body) = serde_json::from_slice::<Value>(body) else {
        return;
    };
    event.tool_use.set_terminal_response(&body);
    let Some(usage) = find_usage(&body) else {
        return;
    };
    apply_usage(event, usage);
    event.applied_service_tier = response_service_tier(&body);
}

pub(super) fn response_service_tier(value: &Value) -> Option<DefaultServiceTier> {
    std::iter::successors(Some(value), |value| value.get("response"))
        .take(3)
        .find_map(|value| value.get("service_tier").and_then(Value::as_str))
        .and_then(|tier| match tier.to_ascii_lowercase().as_str() {
            "priority" | "fast" => Some(DefaultServiceTier::Fast),
            "default" | "standard" => Some(DefaultServiceTier::Standard),
            _ => None,
        })
}

pub(super) fn emit_usage(runtime: &GatewayRuntime, mut event: UsageEvent) {
    if event.success && event.error_category.is_none() {
        if let Some(origin) = runtime.request_origin(&event.request_id) {
            event.error_category = Some(format!("codex_{origin}"));
        }
    }
    if event
        .account_id
        .as_deref()
        .is_some_and(|account_id| !runtime.account_candidate_is_active(account_id))
    {
        return;
    }
    let observed_at_ms = now_ms();
    runtime.apply_usage_event(&event, observed_at_ms);
    if event.quota_snapshot.is_none() {
        event.quota_snapshot = event.candidate_id.as_deref().and_then(|candidate_id| {
            runtime.take_passive_quota_snapshot(candidate_id, observed_at_ms)
        });
    }
    emit_callback(&runtime.usage, event);
}

pub(super) fn emit_callback(callback: &crate::UsageCallback, event: UsageEvent) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(event)));
}

pub(super) fn completed_account_response(bytes: &[u8]) -> Result<Vec<u8>, AttemptFailure> {
    if serde_json::from_slice::<Value>(bytes).is_ok() {
        return Ok(bytes.to_vec());
    }
    let mut offset = 0;
    let mut output = Vec::new();
    while let Some(end) = sse_event_end(&bytes[offset..]) {
        let terminal = parse_sse_event(&bytes[offset..offset + end]);
        if terminal.has_data && !terminal.valid {
            return Err(AttemptFailure::stream("stream_invalid"));
        }
        if let Some(item) = terminal.output_item {
            output.push(item);
        }
        match terminal.outcome {
            Some(TerminalOutcome::Failure) => {
                let category = terminal.error_category.unwrap_or("upstream_terminal");
                return Err(AttemptFailure::classified_with_hint(
                    terminal
                        .error_status
                        .unwrap_or_else(|| upstream_failure_status(category)),
                    category,
                    terminal.cooldown_hint,
                ));
            }
            Some(TerminalOutcome::Success | TerminalOutcome::Incomplete) => {
                if let Some(mut response) = terminal.response {
                    if response
                        .get("output")
                        .and_then(Value::as_array)
                        .is_some_and(Vec::is_empty)
                    {
                        response["output"] = Value::Array(output);
                    }
                    return serde_json::to_vec(&response)
                        .map_err(|_| AttemptFailure::stream("stream_invalid"));
                }
            }
            None => {}
        }
        offset += end;
    }
    Err(AttemptFailure::stream("stream_incomplete"))
}

pub(super) fn apply_usage(event: &mut UsageEvent, usage: &Value) {
    let reported_input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_u64);
    let anthropic_cache_read_tokens = usage.get("cache_read_input_tokens").and_then(Value::as_u64);
    let anthropic_cache_write_tokens = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64);
    let input_tokens =
        if anthropic_cache_read_tokens.is_some() || anthropic_cache_write_tokens.is_some() {
            Some(
                reported_input_tokens
                    .unwrap_or_default()
                    .saturating_add(anthropic_cache_read_tokens.unwrap_or_default())
                    .saturating_add(anthropic_cache_write_tokens.unwrap_or_default()),
            )
        } else {
            reported_input_tokens
        };
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_u64);
    event.input_tokens = input_tokens;
    event.cached_input_tokens = usage
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
        })
        .or_else(|| usage.get("cached_tokens"))
        .or_else(|| usage.get("cache_read_input_tokens"))
        .and_then(Value::as_u64)
        .map(|cached| cached.min(input_tokens.unwrap_or(cached)));
    event.cache_write_input_tokens = usage
        .get("input_tokens_details")
        .and_then(|details| details.get("cache_write_tokens"))
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cache_write_tokens"))
        })
        .or_else(|| usage.get("cache_write_tokens"))
        .or_else(|| usage.get("cache_creation_input_tokens"))
        .and_then(Value::as_u64)
        .map(|written| {
            written.min(
                input_tokens
                    .unwrap_or(written)
                    .saturating_sub(event.cached_input_tokens.unwrap_or_default()),
            )
        });
    event.cache_write_ttl = event
        .cache_write_input_tokens
        .filter(|written| *written > 0)
        .and_then(|_| cache_write_ttl_from_usage(usage).or(event.cache_write_ttl));
    event.reasoning_tokens = usage
        .get("reasoning_tokens")
        .or_else(|| {
            usage
                .get("output_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
        })
        .or_else(|| {
            usage
                .get("completion_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
        })
        .and_then(Value::as_u64)
        .map(|reasoning| reasoning.min(output_tokens.unwrap_or(reasoning)));
    event.output_tokens = output_tokens;
    let reported_total = usage.get("total_tokens").and_then(Value::as_u64);
    event.total_tokens = match (input_tokens, output_tokens) {
        (Some(input), Some(output)) => {
            let measured = input.saturating_add(output);
            Some(reported_total.unwrap_or(measured).max(measured))
        }
        _ => reported_total,
    };
}

fn cache_write_ttl_from_usage(usage: &Value) -> Option<CacheWriteTtl> {
    usage
        .get("input_tokens_details")
        .and_then(|details| details.get("cache_write_ttl"))
        .or_else(|| usage.get("cache_write_ttl"))
        .and_then(Value::as_str)
        .and_then(CacheWriteTtl::from_anthropic_ttl)
        .or_else(|| {
            let creation = usage.get("cache_creation")?;
            creation
                .get("ephemeral_1h_input_tokens")
                .and_then(Value::as_u64)
                .filter(|tokens| *tokens > 0)
                .map(|_| CacheWriteTtl::OneHour)
                .or_else(|| {
                    creation
                        .get("ephemeral_5m_input_tokens")
                        .and_then(Value::as_u64)
                        .filter(|tokens| *tokens > 0)
                        .map(|_| CacheWriteTtl::FiveMinutes)
                })
        })
}

pub(super) fn find_usage(value: &Value) -> Option<&Value> {
    value.get("usage").or_else(|| {
        let response = value.get("response")?;
        response.get("usage").or_else(|| {
            response
                .get("response")
                .and_then(|nested| nested.get("usage"))
        })
    })
}

pub(super) fn response_id(value: &Value) -> Option<&str> {
    value
        .pointer("/response/response/id")
        .or_else(|| value.pointer("/response/id"))
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(super) fn response_id_from_bytes(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| response_id(&value).map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::test_support::test_usage_event;

    #[test]
    fn non_stream_usage_normalizes_cached_reasoning_and_total_tokens() {
        let mut event = test_usage_event();
        populate_tokens(
            &mut event,
            br#"{"response":{"response":{"service_tier":"priority","usage":{"input_tokens":16,"input_tokens_details":{"cached_tokens":30},"output_tokens":5,"output_tokens_details":{"reasoning_tokens":30},"total_tokens":10}}}}"#,
        );

        assert_eq!(event.input_tokens, Some(16));
        assert_eq!(event.cached_input_tokens, Some(16));
        assert_eq!(event.reasoning_tokens, Some(5));
        assert_eq!(event.output_tokens, Some(5));
        assert_eq!(event.total_tokens, Some(21));
        assert_eq!(event.applied_service_tier, Some(DefaultServiceTier::Fast));
    }

    #[test]
    fn anthropic_usage_adds_cache_read_and_creation_to_total_input() {
        let mut event = test_usage_event();
        populate_tokens(
            &mut event,
            br#"{"usage":{"input_tokens":100,"cache_read_input_tokens":40,"cache_creation_input_tokens":20,"output_tokens":10}}"#,
        );

        assert_eq!(event.input_tokens, Some(160));
        assert_eq!(event.cached_input_tokens, Some(40));
        assert_eq!(event.cache_write_input_tokens, Some(20));
        assert_eq!(event.output_tokens, Some(10));
        assert_eq!(event.total_tokens, Some(170));
    }

    #[test]
    fn anthropic_cache_creation_reports_actual_write_lifetime() {
        let mut event = test_usage_event();
        populate_tokens(
            &mut event,
            br#"{"usage":{"input_tokens":100,"cache_creation_input_tokens":20,"cache_creation":{"ephemeral_1h_input_tokens":20},"output_tokens":10}}"#,
        );
        assert_eq!(event.cache_write_ttl, Some(CacheWriteTtl::OneHour));

        populate_tokens(
            &mut event,
            br#"{"usage":{"input_tokens":100,"cache_creation_input_tokens":20,"cache_creation":{"ephemeral_5m_input_tokens":20},"output_tokens":10}}"#,
        );
        assert_eq!(event.cache_write_ttl, Some(CacheWriteTtl::FiveMinutes));
    }

    #[test]
    fn proxy_keeps_safe_native_retry_and_request_headers_only() {
        let mut upstream = reqwest::header::HeaderMap::new();
        upstream.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        upstream.insert("retry-after", HeaderValue::from_static("12"));
        upstream.insert("request-id", HeaderValue::from_static("req_native"));
        upstream.insert(
            "x-codex-turn-state",
            HeaderValue::from_static("account-scoped-state"),
        );
        upstream.insert(
            "anthropic-ratelimit-requests-reset",
            HeaderValue::from_static("2026-08-02T00:00:00Z"),
        );
        upstream.insert("authorization", HeaderValue::from_static("Bearer secret"));
        upstream.insert("set-cookie", HeaderValue::from_static("session=secret"));
        upstream.insert("server", HeaderValue::from_static("provider-internal"));

        let response = proxy_response(
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            &upstream,
            Body::empty(),
        );
        assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
        assert_eq!(response.headers()["retry-after"], "12");
        assert_eq!(response.headers()["request-id"], "req_native");
        assert!(response.headers().get("x-codex-turn-state").is_none());
        assert_eq!(
            response.headers()["anthropic-ratelimit-requests-reset"],
            "2026-08-02T00:00:00Z"
        );
        assert!(response.headers().get("authorization").is_none());
        assert!(response.headers().get("set-cookie").is_none());
        assert!(response.headers().get("server").is_none());
    }
}

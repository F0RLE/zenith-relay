use super::errors::{api_error, upstream_failure_status, AttemptFailure, RateLimitBodyHint};
use super::now_ms;
use super::streaming::{parse_sse_event, sse_event_end, TerminalOutcome};
use crate::runtime::{DefaultServiceTier, ExecutorRoute};
use crate::{Error, GatewayRuntime, UsageEvent};
use axum::body::Body;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderValue, Response, StatusCode};
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
    event.error_category = Some(if too_large {
        "upstream_body_too_large".to_string()
    } else {
        "upstream_body".to_string()
    });
    event.latency_ms = started.elapsed().as_millis() as u64;
    emit_usage(runtime, event);
    api_error(
        StatusCode::BAD_GATEWAY,
        if too_large {
            "upstream response is too large"
        } else {
            "upstream response failed"
        },
        "upstream_error",
    )
}

pub(super) fn proxy_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder().status(status).body(body).unwrap();
    for name in [CONTENT_TYPE, CACHE_CONTROL] {
        if let Some(value) = upstream_headers.get(name.as_str()) {
            response.headers_mut().insert(name, value.clone());
        }
    }
    response
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
    if let Some(value) = upstream_headers.get(CACHE_CONTROL.as_str()) {
        response.headers_mut().insert(CACHE_CONTROL, value.clone());
    }
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
    if let Some(value) = upstream_headers.get(CACHE_CONTROL.as_str()) {
        response.headers_mut().insert(CACHE_CONTROL, value.clone());
    }
    response
}

#[allow(clippy::too_many_arguments)]
pub(super) fn usage_event(
    request_id: &str,
    attempt: u16,
    local_key_id: &str,
    route: &ExecutorRoute,
    requested_model: &str,
    success: bool,
    http_status: u16,
    error_category: Option<String>,
    latency_ms: u64,
) -> UsageEvent {
    UsageEvent {
        request_id: request_id.to_string(),
        attempt,
        local_key_id: local_key_id.to_string(),
        source_id: route.source_id.clone(),
        candidate_id: Some(route.candidate_id.clone()),
        account_id: route.account_id.clone(),
        routing: route.routing.clone(),
        requested_model: Some(requested_model.to_string()),
        resolved_model: Some(route.source_model.clone()),
        wire_api: route.wire_api,
        service_tier: route.service_tier,
        applied_service_tier: None,
        success,
        http_status,
        error_category,
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: None,
        latency_ms,
        ttft_ms: None,
        generation_ms: None,
        input_tokens: None,
        cached_input_tokens: None,
        cache_write_input_tokens: None,
        reasoning_tokens: None,
        output_tokens: None,
        total_tokens: None,
        quota_snapshot: None,
    }
}

pub(super) fn populate_tokens(event: &mut UsageEvent, body: &[u8]) {
    let Ok(body) = serde_json::from_slice::<Value>(body) else {
        return;
    };
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
            Some(TerminalOutcome::Success) => {
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
}

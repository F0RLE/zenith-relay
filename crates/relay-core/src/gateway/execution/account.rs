use super::super::errors::{
    api_error, apply_attempt_failure_cooldown, apply_cooldown_for_model,
    apply_failure_cooldown_with_body, apply_failure_state, cooldown_error,
    preserved_upstream_error, previous_response_not_found, recoverable_response_affinity_miss,
    responses_function_item_id_requires_fc_prefix, responses_message_item_id_requires_msg_prefix,
    retry_candidate_limit, retryable_failure, AttemptFailure, CooldownContext,
    PreservedUpstreamError, TRANSIENT_COOLDOWN_MS,
};
use super::super::now_ms;
use super::super::request::{
    account_endpoint_url, forwarded_codex_headers, request_id, request_service_tier,
    tool_use_diagnostics, try_recover_encrypted_content, with_forwarded_tool_diagnostics,
    AccountEndpoint, CODEX_RESPONSES_LITE_HEADER,
};
use super::super::response::{emit_usage, populate_tokens, proxy_response, usage_event};
use crate::protocol::{remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids};
use crate::runtime::AuthenticatedKey;
use crate::{GatewayRuntime, WireApi};
use axum::body::Body;
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use serde_json::Value;
use std::sync::Arc;
use std::time::Instant;

#[allow(clippy::too_many_arguments)]
pub(in crate::gateway) async fn execute_account_endpoint(
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    mut request: Value,
    requested_model: String,
    resolved_model: String,
    client_headers: HeaderMap,
    endpoint: AccountEndpoint,
    responses_lite: Option<HeaderValue>,
    response_affinity_key: Option<String>,
    rewrite_model: bool,
) -> Response<Body> {
    let request_id = request_id();
    let service_tier = request_service_tier(&request);
    let client_tool_use = tool_use_diagnostics(&request);
    let prompt_affinity_key = runtime.prompt_affinity_key(
        &key.id,
        &resolved_model,
        request.get("prompt_cache_key").and_then(Value::as_str),
    );
    let has_previous_response_id = request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let account_only_exclusions = runtime.api_source_candidate_ids();
    let mut tried = account_only_exclusions.clone();
    let mut attempt = 0_u16;
    let mut owner_recovery_confirmed = false;
    let mut encrypted_content_recovered = false;
    let mut function_item_id_repair_attempted = false;
    let mut message_item_id_repair_attempted = false;
    let mut last_failure = None;
    let mut last_preserved_upstream_error: Option<PreservedUpstreamError> = None;

    while usize::from(attempt)
        < retry_candidate_limit(runtime.max_retry_candidates(), owner_recovery_confirmed)
            + usize::from(encrypted_content_recovered)
    {
        let Some((selected, lease)) = runtime
            .select_and_reserve(
                &key,
                &resolved_model,
                &[WireApi::Responses],
                &tried,
                (
                    response_affinity_key.as_deref(),
                    prompt_affinity_key.as_deref(),
                ),
                now_ms(),
            )
            .await
        else {
            break;
        };
        tried.insert(selected.candidate_id.clone());
        let response_affinity_hit = selected.response_affinity_hit;
        let Some(mut route) = runtime.executor_route(
            &selected.candidate_id,
            &resolved_model,
            &key.scope_snapshot(),
            &[WireApi::Responses],
        ) else {
            continue;
        };
        if route.account_id.is_none() {
            continue;
        }
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.service_tier = service_tier;
        let cooldown_context = CooldownContext {
            scope: &route.scope,
            allowed_protocols: &route.allowed_protocols,
        };
        let Some(upstream_url) = account_endpoint_url(route.upstream_url.clone(), endpoint) else {
            last_failure = Some(AttemptFailure::invalid_request());
            continue;
        };
        let mut upstream_body = request.clone();
        if rewrite_model {
            upstream_body.as_object_mut().unwrap().insert(
                "model".to_string(),
                Value::String(route.source_model.clone()),
            );
        }
        let Ok(request_body) = serde_json::to_vec(&upstream_body) else {
            return api_error(
                StatusCode::BAD_REQUEST,
                "request body could not be serialized",
                "invalid_request",
            );
        };
        let tool_use = with_forwarded_tool_diagnostics(&client_tool_use, &request_body);

        attempt = attempt.saturating_add(1);
        let started = Instant::now();
        let mut upstream_request = runtime
            .request_client(&route.candidate_id, false)
            .post(upstream_url)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .headers(forwarded_codex_headers(&client_headers, &request_id));
        if endpoint == AccountEndpoint::Compact {
            if let Some(value) = responses_lite.as_ref() {
                upstream_request =
                    upstream_request.header(CODEX_RESPONSES_LITE_HEADER, value.clone());
            }
        }
        let upstream = runtime
            .send_authorized_request(
                &route.candidate_id,
                upstream_request.body(request_body),
                None,
            )
            .await;
        let upstream = match upstream {
            Ok(upstream) => upstream,
            Err(error) => {
                let failure = AttemptFailure::authorized_request(error);
                let state = apply_attempt_failure_cooldown(
                    &runtime,
                    &route.candidate_id,
                    &route.source_model,
                    &failure,
                    &HeaderMap::new(),
                    &cooldown_context,
                    route.half_open_probe,
                );
                let mut event = usage_event(
                    &request_id,
                    attempt,
                    &key.id,
                    &route,
                    &requested_model,
                    false,
                    failure.status.as_u16(),
                    Some(failure.category.to_string()),
                    started.elapsed().as_millis() as u64,
                    tool_use.clone(),
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
        };
        let status = upstream.status();
        let response_headers = upstream.headers().clone();
        let Ok(bytes) =
            crate::transport::collect_limited(upstream, endpoint.response_limit()).await
        else {
            let failure = AttemptFailure::body();
            let state = apply_cooldown_for_model(
                &runtime,
                &route.candidate_id,
                "*",
                &route.source_model,
                TRANSIENT_COOLDOWN_MS,
                &cooldown_context,
                route.half_open_probe,
            );
            let mut event = usage_event(
                &request_id,
                attempt,
                &key.id,
                &route,
                &requested_model,
                false,
                failure.status.as_u16(),
                Some(failure.category.to_string()),
                started.elapsed().as_millis() as u64,
                tool_use.clone(),
            );
            apply_failure_state(&mut event, state);
            emit_usage(&runtime, event);
            last_failure = Some(failure);
            continue;
        };
        if !status.is_success() {
            if !function_item_id_repair_attempted
                && responses_function_item_id_requires_fc_prefix(&bytes)
                && repair_call_prefixed_function_item_ids(&mut request)
            {
                function_item_id_repair_attempted = true;
                attempt = attempt.saturating_sub(1);
                tried.remove(&route.candidate_id);
                continue;
            }
            if !message_item_id_repair_attempted
                && responses_message_item_id_requires_msg_prefix(&bytes)
                && remove_item_prefixed_message_ids(&mut request)
            {
                message_item_id_repair_attempted = true;
                attempt = attempt.saturating_sub(1);
                tried.remove(&route.candidate_id);
                continue;
            }
            let failure = AttemptFailure::status_with_body(status, Some(&bytes));
            last_preserved_upstream_error = preserved_upstream_error(&failure, &bytes);
            let mut event = usage_event(
                &request_id,
                attempt,
                &key.id,
                &route,
                &requested_model,
                false,
                status.as_u16(),
                Some(failure.category.to_string()),
                started.elapsed().as_millis() as u64,
                tool_use.clone(),
            );
            if failure.category == "upstream_encrypted_content_invalid"
                && try_recover_encrypted_content(&mut request, &mut encrypted_content_recovered)
            {
                tried.remove(&route.candidate_id);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
            let affinity_miss = recoverable_response_affinity_miss(
                status,
                has_previous_response_id,
                response_affinity_hit,
                previous_response_not_found(&bytes),
            );
            if affinity_miss
                || retryable_failure(status, failure.category, has_previous_response_id)
            {
                if affinity_miss {
                    owner_recovery_confirmed |= !response_affinity_hit;
                    runtime.invalidate_response_affinity(response_affinity_key.as_deref());
                    event.error_category = Some("response_affinity_miss".to_string());
                } else {
                    let state = apply_failure_cooldown_with_body(
                        &runtime,
                        &route.candidate_id,
                        &route.source_model,
                        status,
                        failure.category,
                        &response_headers,
                        Some(&bytes),
                        &cooldown_context,
                        route.half_open_probe,
                    );
                    apply_failure_state(&mut event, state);
                }
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
            emit_usage(&runtime, event);
            return proxy_response(status, &response_headers, Body::from(bytes));
        }

        let mut event = usage_event(
            &request_id,
            attempt,
            &key.id,
            &route,
            &requested_model,
            true,
            status.as_u16(),
            None,
            started.elapsed().as_millis() as u64,
            tool_use,
        );
        populate_tokens(&mut event, &bytes);
        let recovered = runtime.record_success_with_metrics(
            &route.candidate_id,
            &route.source_model,
            now_ms(),
            event.output_tokens,
            event.generation_ms.unwrap_or(event.latency_ms),
        );
        event.consecutive_failures = recovered.then_some(0);
        runtime.bind_prompt_affinity(
            prompt_affinity_key.as_deref(),
            &route.candidate_id,
            now_ms(),
        );
        emit_usage(&runtime, event);
        drop(lease);
        return proxy_response(status, &response_headers, Body::from(bytes));
    }

    let failure = last_failure.unwrap_or_else(AttemptFailure::no_candidate);
    if failure.status == StatusCode::TOO_MANY_REQUESTS {
        if let Some((retry_at, reason)) = runtime.all_applicable_cooldown(
            &key,
            &resolved_model,
            &[WireApi::Responses],
            &account_only_exclusions,
            response_affinity_key.as_deref(),
            now_ms(),
        ) {
            return cooldown_error(
                retry_at,
                Some(&failure),
                reason == crate::scheduler::CooldownReason::RateLimit,
            );
        }
    }
    if let Some(preserved) = last_preserved_upstream_error.as_ref().filter(|preserved| {
        preserved.status == failure.status && preserved.category == failure.category
    }) {
        return api_error(preserved.status, &preserved.message, &preserved.code);
    }
    api_error(failure.status, failure.message, failure.category)
}

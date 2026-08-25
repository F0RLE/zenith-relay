use super::super::errors::{
    api_error, api_error_with_origin, api_error_with_origin_and_category,
    apply_attempt_failure_cooldown, apply_cooldown_for_model, apply_failure_cooldown_with_body,
    apply_failure_state, cooldown_error, preserved_upstream_error, previous_response_not_found,
    recoverable_response_affinity_miss, responses_function_item_id_requires_fc_prefix,
    responses_message_item_id_requires_msg_prefix, retry_candidate_limit, retryable_failure,
    AttemptFailure, CooldownContext, PreservedUpstreamError, TRANSIENT_COOLDOWN_MS,
};
use super::super::now_ms;
use super::super::request::{
    account_endpoint_url, apply_default_service_tier_if_missing, client_context_fingerprint,
    forwarded_codex_headers, request_id, request_service_tier, tool_use_diagnostics,
    try_recover_encrypted_content, with_forwarded_tool_diagnostics, AccountEndpoint,
    CODEX_RESPONSES_LITE_HEADER,
};
use super::super::response::{
    emit_usage, populate_tokens, proxy_error_response, proxy_response, route_error_origin,
    usage_event,
};
use super::super::turn_state::{guard_account_request, relay_account_response_header};
use crate::protocol::{remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids};
use crate::runtime::AuthenticatedKey;
use crate::usage::ReasoningEffortDiagnostics;
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
    let client_supplied_service_tier = request.get("service_tier").is_some();
    let request_id = request_id();
    let service_tier = request_service_tier(&request);
    let client_tool_use = tool_use_diagnostics(&request);
    let client_context_id = client_context_fingerprint(&client_headers);
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
    let mut last_failure_origin = crate::ErrorOrigin::Relay;

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
            false,
        ) else {
            continue;
        };
        if route.account_id.is_none() {
            continue;
        }
        if !client_supplied_service_tier {
            request
                .as_object_mut()
                .expect("request object was validated before routing")
                .remove("service_tier");
        }
        apply_default_service_tier_if_missing(
            &mut request,
            runtime.model_service_tier_for_candidate(&route.candidate_id, &route.source_model),
        );
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.client_context_id = client_context_id.clone();
        route.service_tier = service_tier;
        let route_responses_lite = responses_lite.clone().or_else(|| {
            route
                .account_id
                .as_deref()
                .is_some_and(|candidate_id| {
                    runtime
                        .codex_model_responses_lite_candidates(&resolved_model)
                        .iter()
                        .any(|id| id == candidate_id)
                })
                .then(|| HeaderValue::from_static("true"))
        });
        let selected_error_origin = route_error_origin(&route);
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
        if route_responses_lite.is_some() {
            if let Some(object) = upstream_body.as_object_mut() {
                crate::gateway::request::normalize_account_request(object, true);
                // Responses Lite is also used by the compact endpoint, but
                // compact remains a non-streaming contract. The shared
                // account normalizer sets the native streaming defaults, so
                // remove that field again for this endpoint only.
                if endpoint == AccountEndpoint::Compact {
                    object.remove("stream");
                }
            }
        }
        let reasoning_effort =
            ReasoningEffortDiagnostics::from_bodies(&request, &upstream_body, WireApi::Responses);
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
        let mut request_headers = forwarded_codex_headers(&client_headers, &request_id);
        guard_account_request(
            &runtime,
            &key.id,
            &mut request_headers,
            route.account_id.as_deref().unwrap_or_default(),
            now_ms(),
        );
        let mut upstream_request = runtime
            .request_client(&route.candidate_id, false)
            .post(upstream_url)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .headers(request_headers);
        if endpoint == AccountEndpoint::Compact {
            if let Some(value) = route_responses_lite.as_ref() {
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
                    Some(&reasoning_effort),
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
                last_failure_origin = selected_error_origin;
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
                Some(&reasoning_effort),
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
            last_failure_origin = selected_error_origin;
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
                Some(&reasoning_effort),
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
                last_failure_origin = selected_error_origin;
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
                last_failure_origin = selected_error_origin;
                continue;
            }
            emit_usage(&runtime, event);
            let mut response = proxy_error_response(
                status,
                &response_headers,
                Body::from(bytes),
                selected_error_origin,
                failure.category,
                Some(&request_id),
            );
            if let Some(account_id) = route.account_id.as_deref() {
                relay_account_response_header(
                    &runtime,
                    &key.id,
                    &client_headers,
                    account_id,
                    &response_headers,
                    &mut response,
                    now_ms(),
                );
            }
            return response;
        }

        let mut event = usage_event(
            &request_id,
            attempt,
            &key.id,
            &route,
            Some(&reasoning_effort),
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
        let mut response = proxy_response(status, &response_headers, Body::from(bytes));
        if let Some(account_id) = route.account_id.as_deref() {
            relay_account_response_header(
                &runtime,
                &key.id,
                &client_headers,
                account_id,
                &response_headers,
                &mut response,
                now_ms(),
            );
        }
        return response;
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
        return api_error_with_origin_and_category(
            preserved.status,
            &preserved.message,
            &preserved.code,
            preserved.category,
            last_failure_origin,
            Some(&request_id),
        );
    }
    api_error_with_origin(
        failure.status,
        failure.message,
        failure.category,
        last_failure_origin,
        Some(&request_id),
    )
}

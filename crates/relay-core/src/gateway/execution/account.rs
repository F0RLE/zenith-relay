use super::super::continuation::{
    drop_materialized_previous_response_id, prepare_response_continuation,
    RESPONSE_CONTINUATION_UNAVAILABLE_CODE, RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
};
use super::super::errors::{
    api_error, apply_attempt_failure_cooldown, apply_cooldown_for_model,
    apply_failure_cooldown_with_body, apply_failure_state, is_deactivated_workspace,
    preserved_upstream_error, previous_response_not_found, prompt_cache_write_rejected,
    recoverable_response_affinity_miss, recoverable_response_model_switch,
    responses_call_id_is_missing, responses_custom_tool_item_id_requires_ctc_prefix,
    responses_function_item_id_requires_fc_prefix, responses_message_item_id_requires_msg_prefix,
    responses_tool_call_is_missing_output, retryable_failure, AttemptFailure, CooldownContext,
    PreservedUpstreamError, TRANSIENT_COOLDOWN_MS,
};
use super::super::now_ms;
use super::super::request::{
    account_endpoint_url, apply_codex_routing_hint, client_context_fingerprint,
    codex_client_version, forwarded_codex_headers, repair_legacy_responses_call_ids, request_id,
    response_tool_call_ids, responses_lite_parallel_tool_calls_valid, tool_use_diagnostics,
    unpaired_tool_output_ids, with_forwarded_tool_diagnostics, AccountEndpoint, ServiceTierPolicy,
    CODEX_RESPONSES_LITE_HEADER,
};
use super::super::response::{
    emit_usage, populate_tokens, proxy_error_response, proxy_response, response_id_from_bytes,
    route_error_origin, usage_event,
};
use super::super::turn_state::{guard_account_request, relay_account_response_header};
use super::finish_request_failure;
use super::request::should_wait_for_candidate_availability;
use super::{wait_for_candidate_retry, AutomaticRecovery, CandidateRetryContext};
use crate::error_codes;
use crate::protocol::{
    remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids,
    repair_custom_tool_item_ids,
};
use crate::runtime::AuthenticatedKey;
use crate::usage::ReasoningEffortDiagnostics;
use crate::{GatewayRuntime, WireApi};
use axum::body::Body;
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

pub(in crate::gateway) struct AccountExecution {
    pub(in crate::gateway) runtime: Arc<GatewayRuntime>,
    pub(in crate::gateway) key: AuthenticatedKey,
    pub(in crate::gateway) request: Value,
    pub(in crate::gateway) requested_model: String,
    pub(in crate::gateway) resolved_model: String,
    pub(in crate::gateway) client_headers: HeaderMap,
    pub(in crate::gateway) endpoint: AccountEndpoint,
    pub(in crate::gateway) responses_lite: Option<HeaderValue>,
    pub(in crate::gateway) rewrite_model: bool,
    pub(in crate::gateway) wait_for_candidate_availability: bool,
    /// Account-only callers such as the background wake path deliberately
    /// disable the automatic Responses Lite contract.  Lite is a whole
    /// request contract and must never be inferred for a synthetic internal
    /// request merely because the account's catalog happened to confirm it.
    pub(in crate::gateway) allow_automatic_responses_lite: bool,
    /// Optional internal origin attached to the request's usage record.  This
    /// keeps scheduler-owned work distinguishable from customer traffic while
    /// preserving the same account execution and retry pipeline.
    pub(in crate::gateway) request_origin: Option<&'static str>,
}

pub(in crate::gateway) async fn execute_account_endpoint(
    context: AccountExecution,
) -> Response<Body> {
    let AccountExecution {
        runtime,
        key,
        mut request,
        requested_model,
        resolved_model,
        client_headers,
        endpoint,
        responses_lite,
        rewrite_model,
        wait_for_candidate_availability,
        allow_automatic_responses_lite,
        request_origin,
    } = context;
    let service_tier_policy = ServiceTierPolicy::pool_owned(&request);
    let request_id = request_id();
    if let Some(origin) = request_origin {
        runtime.mark_request_origin(&request_id, origin);
    }
    let client_tool_use = tool_use_diagnostics(&request);
    let client_context_id = client_context_fingerprint(&client_headers);
    let prompt_affinity_key = runtime.prompt_affinity_key(
        &key.id,
        &resolved_model,
        request.get("prompt_cache_key").and_then(Value::as_str),
        client_context_id.as_deref(),
    );
    let continuation =
        match prepare_response_continuation(&runtime, &key.id, &mut request, now_ms(), None) {
            Ok(continuation) => continuation,
            Err(()) => {
                return api_error(
                    StatusCode::CONFLICT,
                    RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
                    RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
                )
            }
        };
    let mut response_affinity_key = continuation.response_affinity_key;
    let mut requires_affinity_owner = continuation.requires_affinity_owner;
    let mut has_unpaired_tool_output = continuation.has_unpaired_tool_output;
    let account_only_exclusions = runtime.api_source_candidate_ids();
    let mut tried = account_only_exclusions.clone();
    let mut attempt = 0_u16;
    let mut function_item_id_repair_attempted = false;
    let mut custom_tool_item_id_repair_attempted = false;
    let mut message_item_id_repair_attempted = false;
    let mut legacy_call_id_repair_attempted = false;
    let mut model_switch_reset_attempted = false;
    let mut stale_tool_history_recovered = false;
    let mut last_failure: Option<AttemptFailure> = None;
    let mut last_preserved_upstream_error: Option<PreservedUpstreamError> = None;
    let mut last_failure_origin = crate::ErrorOrigin::Relay;
    let mut retry_deadline = (!runtime.chatgpt_retry_until_available()).then(|| {
        tokio::time::Instant::now() + Duration::from_millis(runtime.chatgpt_retry_window_ms())
    });
    let mut retry_wait_attempt = 0u32;
    let mut retry_window_expired = false;
    let mut automatic_recovery = AutomaticRecovery::new();
    let retry_context = CandidateRetryContext {
        runtime: &runtime,
        key: &key,
        resolved_model: &resolved_model,
        protocols: &[WireApi::Responses],
        exclusions: &account_only_exclusions,
    };
    // Compact and search endpoints only select OAuth accounts, but their
    // retries can still move between account slots. Keep automatic Lite off
    // unless every such configured slot confirmed the same contract.
    let automatic_responses_lite = allow_automatic_responses_lite
        && runtime.codex_model_account_responses_routes_all_support_lite(&key, &resolved_model);

    loop {
        // A model-switch or stale-tool recovery can remove the opaque
        // continuation id. Retry policy must then use the repaired request,
        // not the continuation state captured before the loop.
        let has_previous_response_id = request_has_previous_response_id(&request);
        // Account-only endpoints are eligible for the managed ChatGPT policy,
        // but its enabled state may change while this request waits.
        let retry_until_available = runtime.chatgpt_retry_until_available();
        // An eligible request can outlive a toggle change. Dropping a finite
        // deadline here lets an operator turn persistent recovery on while it
        // is waiting instead of preserving the old bounded policy.
        if retry_until_available {
            retry_deadline = None;
        }
        let wait_for_candidate_availability =
            wait_for_candidate_availability && retry_until_available;
        let attempt_limit = runtime.max_retry_candidates();
        if usize::from(attempt) >= attempt_limit {
            if should_wait_for_candidate_availability(
                wait_for_candidate_availability,
                &last_failure,
                false,
                has_previous_response_id,
            ) {
                attempt = 0;
                let backoff = account_retry_backoff(retry_wait_attempt.saturating_add(1));
                if !wait_for_candidate_retry(
                    &retry_context,
                    &mut tried,
                    response_affinity_key.as_deref(),
                    &mut retry_wait_attempt,
                    backoff,
                    retry_deadline,
                )
                .await
                {
                    retry_window_expired = true;
                    break;
                }
                continue;
            }
            break;
        }
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
            if !requires_affinity_owner
                && runtime.release_unroutable_response_affinity(
                    &key,
                    &mut response_affinity_key,
                    &resolved_model,
                    &[WireApi::Responses],
                    now_ms(),
                )
            {
                continue;
            }
            // Account-only continuations are pinned to their creating account
            // while it remains in the key scope. If pool membership removes
            // that owner, drop the opaque response id once so another account
            // can continue the chat instead of waiting forever on an
            // impossible affinity selection. Temporary health, quota, and
            // cooldown misses remain retryable on the original owner.
            if has_previous_response_id
                && !has_unpaired_tool_output
                && !model_switch_reset_attempted
                && response_affinity_key.as_deref().and_then(|affinity_key| {
                    runtime.response_affinity_owner_supports_route(
                        &key,
                        affinity_key,
                        &resolved_model,
                        &[WireApi::Responses],
                        now_ms(),
                    )
                }) == Some(false)
                && drop_materialized_previous_response_id(
                    &runtime,
                    &key.id,
                    &mut request,
                    &resolved_model,
                    now_ms(),
                )
            {
                model_switch_reset_attempted = true;
                response_affinity_key = None;
                requires_affinity_owner = false;
                continue;
            }
            if automatic_recovery
                .retry(&retry_context, &mut tried, response_affinity_key.as_deref())
                .await
            {
                continue;
            }
            if should_wait_for_candidate_availability(
                wait_for_candidate_availability,
                &last_failure,
                false,
                has_previous_response_id,
            ) {
                let backoff = account_retry_backoff(retry_wait_attempt.saturating_add(1));
                if !wait_for_candidate_retry(
                    &retry_context,
                    &mut tried,
                    response_affinity_key.as_deref(),
                    &mut retry_wait_attempt,
                    backoff,
                    retry_deadline,
                )
                .await
                {
                    retry_window_expired = true;
                    break;
                }
                continue;
            }
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
        let selected_service_tier =
            runtime.model_service_tier_for_candidate(&route.candidate_id, &route.source_model);
        service_tier_policy.prepare_for_candidate(
            &mut request,
            selected_service_tier,
            WireApi::Responses,
        );
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.client_context_id = client_context_id.clone();
        route.service_tier =
            service_tier_policy.effective_tier(&request, selected_service_tier, WireApi::Responses);
        let route_responses_lite = responses_lite.clone().or_else(|| {
            (automatic_responses_lite
                && route.account_id.as_deref().is_some_and(|candidate_id| {
                    runtime
                        .codex_model_responses_lite_candidates(&resolved_model)
                        .iter()
                        .any(|id| id == candidate_id)
                }))
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
                if !responses_lite_parallel_tool_calls_valid(object) {
                    return api_error(
                        StatusCode::BAD_REQUEST,
                        "responses Lite requires parallel_tool_calls to be a boolean",
                        error_codes::INVALID_REQUEST,
                    );
                }
                if endpoint == AccountEndpoint::Compact {
                    crate::gateway::request::normalize_compact_account_request(object, true);
                } else {
                    crate::gateway::request::normalize_account_request(object, true);
                }
            }
        }
        let reasoning_effort =
            ReasoningEffortDiagnostics::from_bodies(&request, &upstream_body, WireApi::Responses);
        let Ok(request_body) = serde_json::to_vec(&upstream_body) else {
            return api_error(
                StatusCode::BAD_REQUEST,
                "request body could not be serialized",
                error_codes::INVALID_REQUEST,
            );
        };
        let tool_use = with_forwarded_tool_diagnostics(&client_tool_use, &request_body);

        attempt = attempt.saturating_add(1);
        let started = Instant::now();
        let mut request_headers = forwarded_codex_headers(&client_headers, &request_id);
        apply_codex_routing_hint(
            &mut request_headers,
            &route.source_model,
            route.service_tier,
        );
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
                codex_client_version(&client_headers),
            )
            .await;
        let upstream = match upstream {
            Ok(upstream) => {
                route.account_token_generation = upstream.account_token_generation;
                upstream.response
            }
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
            if !legacy_call_id_repair_attempted
                && responses_call_id_is_missing(&bytes)
                && repair_legacy_responses_call_ids(&mut request)
            {
                legacy_call_id_repair_attempted = true;
                attempt = attempt.saturating_sub(1);
                tried.remove(&route.candidate_id);
                has_unpaired_tool_output = !unpaired_tool_output_ids(&request).is_empty();
                requires_affinity_owner =
                    request_has_previous_response_id(&request) || has_unpaired_tool_output;
                continue;
            }
            if !function_item_id_repair_attempted
                && responses_function_item_id_requires_fc_prefix(&bytes)
                && repair_call_prefixed_function_item_ids(&mut request)
            {
                function_item_id_repair_attempted = true;
                attempt = attempt.saturating_sub(1);
                tried.remove(&route.candidate_id);
                continue;
            }
            if !custom_tool_item_id_repair_attempted
                && responses_custom_tool_item_id_requires_ctc_prefix(&bytes)
                && repair_custom_tool_item_ids(&mut request)
            {
                custom_tool_item_id_repair_attempted = true;
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
            if status == StatusCode::PAYMENT_REQUIRED
                && route.account_id.is_some()
                && is_deactivated_workspace(&bytes)
            {
                runtime.trip_chatgpt_team_breaker(&route.candidate_id, now_ms());
            }
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
            let cache_write_rejected = prompt_cache_write_rejected(&bytes);
            event.upstream_error = Some(crate::usage::UpstreamErrorDetails::from_body(
                Some(status.as_u16()),
                &bytes,
            ));
            if !stale_tool_history_recovered
                && has_previous_response_id
                && responses_tool_call_is_missing_output(&bytes)
                && drop_materialized_previous_response_id(
                    &runtime,
                    &key.id,
                    &mut request,
                    &resolved_model,
                    now_ms(),
                )
            {
                stale_tool_history_recovered = true;
                response_affinity_key = None;
                requires_affinity_owner = false;
                tried.remove(&route.candidate_id);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                last_failure_origin = selected_error_origin;
                continue;
            }
            if !model_switch_reset_attempted
                && recoverable_response_model_switch(
                    status,
                    failure.category,
                    has_previous_response_id,
                    has_unpaired_tool_output,
                    &bytes,
                )
                && drop_materialized_previous_response_id(
                    &runtime,
                    &key.id,
                    &mut request,
                    &resolved_model,
                    now_ms(),
                )
            {
                model_switch_reset_attempted = true;
                response_affinity_key = None;
                requires_affinity_owner = false;
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
            if affinity_miss {
                event.error_category = Some(error_codes::RESPONSE_AFFINITY_MISS.to_string());
                emit_usage(&runtime, event);
                if !model_switch_reset_attempted
                    && drop_materialized_previous_response_id(
                        &runtime,
                        &key.id,
                        &mut request,
                        &resolved_model,
                        now_ms(),
                    )
                {
                    model_switch_reset_attempted = true;
                    response_affinity_key = None;
                    requires_affinity_owner = false;
                    tried.remove(&route.candidate_id);
                    last_failure = Some(failure);
                    last_failure_origin = selected_error_origin;
                    continue;
                }
                return api_error(
                    StatusCode::CONFLICT,
                    RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
                    RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
                );
            }
            if cache_write_rejected {
                runtime.invalidate_prompt_affinity(prompt_affinity_key.as_deref());
            }
            if cache_write_rejected
                || retryable_failure(status, failure.category, has_previous_response_id)
            {
                if response_affinity_hit && !requires_affinity_owner {
                    response_affinity_key = None;
                }
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
        if let Ok(response) = serde_json::from_slice::<Value>(&bytes) {
            runtime.capture_native_responses_replay(
                &key.id,
                &route.candidate_id,
                &request,
                &route.source_model,
                &response,
                now_ms(),
            );
            for call_id in response_tool_call_ids(&response) {
                runtime.bind_tool_call_affinity(&key.id, &call_id, &route.candidate_id, now_ms());
            }
        }
        runtime.bind_response_affinity(
            response_id_from_bytes(&bytes).as_deref(),
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

    let failure = if retry_window_expired {
        AttemptFailure::classified_with_hint(
            StatusCode::SERVICE_UNAVAILABLE,
            error_codes::UPSTREAM_UNAVAILABLE,
            Default::default(),
        )
    } else {
        last_failure.unwrap_or_else(AttemptFailure::no_candidate)
    };
    finish_request_failure(
        &runtime,
        &key,
        &resolved_model,
        &[WireApi::Responses],
        &account_only_exclusions,
        response_affinity_key.as_deref(),
        failure,
        last_preserved_upstream_error.as_ref(),
        last_failure_origin,
        &request_id,
    )
}

fn account_retry_backoff(attempt: u32) -> Duration {
    let exponent = attempt.min(6);
    let base_ms = 100u64.saturating_mul(1u64 << exponent);
    let jitter_ms = u64::from((attempt.wrapping_mul(37)) % 100);
    Duration::from_millis((base_ms + jitter_ms).min(5_000))
}

fn request_has_previous_response_id(request: &Value) -> bool {
    request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

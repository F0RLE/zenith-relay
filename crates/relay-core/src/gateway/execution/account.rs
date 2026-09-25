use super::super::continuation::{
    drop_materialized_previous_response_id, prepare_response_continuation,
    recover_stale_tool_history as replay_and_prune_stale_tool_history,
    RESPONSE_CONTINUATION_UNAVAILABLE_CODE, RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
};
use super::super::errors::{
    api_error, apply_failure_state, current_failure_state, is_deactivated_workspace,
    preserved_upstream_error, previous_response_not_found, prompt_cache_write_rejected,
    recoverable_response_affinity_miss, recoverable_response_model_switch,
    responses_custom_tool_item_id_requires_ctc_prefix,
    responses_function_item_id_requires_fc_prefix, responses_message_item_id_requires_msg_prefix,
    responses_tool_call_is_missing_output, responses_tool_call_links_rejected, retryable_failure,
    settle_attempt_failure, AttemptFailure, PreservedUpstreamError,
};
use super::super::now_ms;
use super::super::request::{
    account_endpoint_url, apply_codex_routing_hint, client_context_fingerprint,
    codex_client_version, forwarded_codex_headers, is_deferred_tool_search_compatibility_error,
    repair_legacy_responses_call_ids, request_id, response_tool_call_ids,
    responses_lite_parallel_tool_calls_valid, unpaired_tool_output_ids, AccountEndpoint,
    RequestToolPolicy, ServiceTierPolicy, CODEX_RESPONSES_LITE_HEADER,
};
use super::super::response::{
    emit_usage, populate_tokens, proxy_error_response, proxy_response, proxy_sse_response,
    response_id_from_bytes, route_error_origin, usage_event,
};
use super::super::turn_state::{relay_account_response_header, request_scope};
use super::finish_request_failure;
use super::request::{adapter_error_response, should_wait_for_candidate_availability};
use super::{wait_for_candidate_retry, wait_for_recovery, CandidateRetryContext};
use crate::error_codes;
use crate::protocol::{
    remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids,
    repair_custom_tool_item_ids, AdapterError,
};
use crate::runtime::{AccountTransport, AuthenticatedKey, AuthorizedRequestError};
use crate::scheduler::rotation::{ExecutionCertainty, RotationOperation, SharedRequestBudget};
use crate::usage::ReasoningEffortDiagnostics;
use crate::{GatewayRuntime, WireApi};
use axum::body::Body;
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use serde_json::Value;
use std::sync::Arc;
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
    let mut tool_policy = RequestToolPolicy::new(&runtime, &request);
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
    let budget = SharedRequestBudget::for_incoming_request(runtime.request_dispatch_budget());
    let mut function_item_id_repair_attempted = false;
    let mut custom_tool_item_id_repair_attempted = false;
    let mut message_item_id_repair_attempted = false;
    let mut legacy_call_id_repair_attempted = false;
    let mut model_switch_reset_attempted = false;
    let mut stale_tool_history_recovered = false;
    let mut last_failure: Option<AttemptFailure> = None;
    let mut last_adapter_error = None;
    let mut last_preserved_upstream_error: Option<PreservedUpstreamError> = None;
    let mut last_failure_origin = crate::ErrorOrigin::Relay;
    let mut retry_window_expired = false;
    let operation = if endpoint == AccountEndpoint::Compact {
        RotationOperation::Compaction
    } else {
        RotationOperation::Text
    };
    let retry_context = CandidateRetryContext {
        runtime: &runtime,
        key: &key,
        resolved_model: &resolved_model,
        protocols: &[WireApi::Responses],
        operation,
        exclusions: &account_only_exclusions,
    };
    // Compact and search endpoints only select OAuth accounts, but their
    // retries can still move between account slots. Keep automatic Lite off
    // unless every such configured slot confirmed the same contract.
    let automatic_responses_lite = allow_automatic_responses_lite
        && runtime.codex_model_account_responses_routes_all_support_lite(&key, &resolved_model);

    loop {
        budget.retain_input_bytes(crate::gateway::request_body::retained_request_bytes(
            &request,
        ));
        budget.configure_retry_window(
            runtime.route_recovery_window_ms(),
            wait_for_candidate_availability && runtime.route_recovery_enabled(),
        );
        if !budget.can_dispatch() {
            break;
        }
        // A model-switch or stale-tool recovery can remove the opaque
        // continuation id. Retry policy must then use the repaired request,
        // not the continuation state captured before the loop.
        let has_previous_response_id = request_has_previous_response_id(&request);
        // Account-only client endpoints share the API's live route-recovery
        // policy; internal wake requests remain ineligible.
        let retry_until_available = runtime.route_recovery_enabled();
        let wait_for_candidate_availability =
            wait_for_candidate_availability && retry_until_available;
        let Some((selected, lease)) = runtime
            .select_and_reserve_operation_with_budget(
                &key,
                &resolved_model,
                &[WireApi::Responses],
                &tried,
                (
                    response_affinity_key.as_deref(),
                    prompt_affinity_key.as_deref(),
                ),
                now_ms(),
                operation,
                &budget,
            )
            .await
        else {
            if let Some(error) = crate::gateway::errors::admission_error(&budget) {
                return error;
            }
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
            if wait_for_recovery(
                &budget,
                &retry_context,
                &mut tried,
                response_affinity_key.as_deref(),
            )
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
                if !wait_for_candidate_retry(
                    &budget,
                    &retry_context,
                    &mut tried,
                    response_affinity_key.as_deref(),
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
            service_tier_policy.select_for_model(&runtime, &route.source_model);
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
        let basis_points_route = route.account_transport == AccountTransport::ExcelBasisPoints;
        if basis_points_route {
            if endpoint != AccountEndpoint::Wake || responses_lite.is_some() {
                last_adapter_error = Some(AdapterError::parameter_unsupported_for(
                    if endpoint != AccountEndpoint::Wake {
                        "endpoint"
                    } else {
                        "responses_lite"
                    },
                ));
                continue;
            }
            if let Some(error) = super::compatibility::basis_points_route_error(
                &request,
                request.get("stream").and_then(Value::as_bool) == Some(true),
                &service_tier_policy,
                selected_service_tier,
            ) {
                last_adapter_error = Some(error);
                continue;
            }
        }
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
        let upstream_url = if basis_points_route {
            route.upstream_url.clone()
        } else {
            let Some(upstream_url) = account_endpoint_url(route.upstream_url.clone(), endpoint)
            else {
                last_failure = Some(AttemptFailure::invalid_request());
                continue;
            };
            upstream_url
        };
        let mut upstream_body = request.clone();
        if rewrite_model {
            upstream_body.as_object_mut().unwrap().insert(
                "model".to_string(),
                Value::String(route.source_model.clone()),
            );
        }
        if route_responses_lite.is_some() && !basis_points_route {
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
        if basis_points_route {
            if let Some(object) = upstream_body.as_object_mut() {
                crate::gateway::request::normalize_basis_points_request(object);
            }
        }
        // Only the native Responses wake path has the provider contract for
        // `tool_search`. Compact and alpha/search are separate account
        // endpoints and must keep their ordinary full catalog.
        let allow_deferred_tool_search = endpoint == AccountEndpoint::Wake && !basis_points_route;
        if let Err(message) =
            tool_policy.apply_value(&mut upstream_body, allow_deferred_tool_search)
        {
            return api_error(
                StatusCode::BAD_REQUEST,
                message,
                error_codes::INVALID_REQUEST,
            );
        }
        if basis_points_route {
            upstream_body = match super::basis_points::prepare_request(&upstream_body) {
                Ok(prepared) => prepared,
                Err(error) if error.is_route_incompatible() => {
                    last_adapter_error = Some(error);
                    continue;
                }
                Err(error) => return super::request::adapter_error_response(error),
            };
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
        let tool_use = tool_policy.diagnostics.clone();

        let started = Instant::now();
        let failed_usage =
            |route: &crate::runtime::ExecutorRoute, attempt, failure: AttemptFailure| {
                usage_event(
                    &request_id,
                    attempt,
                    &key.id,
                    route,
                    Some(&reasoning_effort),
                    &requested_model,
                    false,
                    failure.status.as_u16(),
                    Some(failure.category.to_string()),
                    started.elapsed().as_millis() as u64,
                    tool_use.clone(),
                )
            };
        let mut request_headers = if basis_points_route {
            route.upstream_headers.clone()
        } else {
            forwarded_codex_headers(&client_headers, &request_id)
        };
        if !basis_points_route {
            apply_codex_routing_hint(
                &mut request_headers,
                &route.source_model,
                route.service_tier,
            );
        }
        let turn_account = route.account_id.clone();
        let turn_model = route.source_model.clone();
        let turn_scope = if basis_points_route {
            None
        } else {
            request_scope(
                &key.id,
                &client_headers,
                turn_account.as_deref(),
                &turn_model,
            )
        };
        let compaction_headers = request_headers.clone();
        let mut upstream_request = runtime
            .request_client(&route.candidate_id)
            .post(upstream_url)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .headers(request_headers);
        if endpoint == AccountEndpoint::Compact && !basis_points_route {
            if let Some(value) = route_responses_lite.as_ref() {
                upstream_request =
                    upstream_request.header(CODEX_RESPONSES_LITE_HEADER, value.clone());
            }
        }
        let upstream = runtime
            .send_authorized_request(
                &route.candidate_id,
                upstream_request.body(request_body),
                (!basis_points_route)
                    .then(|| codex_client_version(&client_headers))
                    .flatten(),
                turn_scope.as_ref(),
                Some(&budget),
                Some(&lease),
            )
            .await;
        let attempt = u16::from(budget.dispatches());
        let upstream = match upstream {
            Ok(upstream) => {
                route.account_token_generation = upstream.account_token_generation;
                upstream.response
            }
            Err(error) => {
                let uncertain = error.execution_certainty() == ExecutionCertainty::Unknown;
                let exhausted = matches!(error, AuthorizedRequestError::DispatchBudgetExhausted);
                let failure = AttemptFailure::authorized_request(error);
                let mut event = failed_usage(&route, attempt, failure);
                if uncertain || exhausted {
                    if uncertain {
                        lease.settle_rotation_unknown(now_ms());
                    }
                    emit_usage(&runtime, event);
                    return super::attempt_error_response(
                        failure,
                        None,
                        selected_error_origin,
                        &request_id,
                    );
                }
                let state = settle_attempt_failure(
                    &runtime,
                    &lease,
                    &route.source_model,
                    &failure,
                    &HeaderMap::new(),
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                last_failure_origin = selected_error_origin;
                continue;
            }
        };
        let mut status = upstream.status();
        let mut response_headers = upstream.headers().clone();
        let Ok(mut bytes) =
            crate::transport::collect_limited(upstream, endpoint.response_limit()).await
        else {
            lease.settle_rotation_unknown(now_ms());
            let failure = AttemptFailure::body();
            let state = current_failure_state(&runtime, &route.candidate_id, &route.source_model);
            let mut event = failed_usage(&route, attempt, failure);
            apply_failure_state(&mut event, state);
            emit_usage(&runtime, event);
            last_failure = Some(failure);
            last_failure_origin = selected_error_origin;
            continue;
        };
        if endpoint == AccountEndpoint::Compact
            && budget.can_dispatch()
            && super::super::compaction::missing_legacy_endpoint(status, &bytes)
        {
            match super::super::compaction::execute(
                &runtime,
                &mut route,
                &upstream_body,
                &compaction_headers,
                turn_scope.as_ref(),
                &budget,
                &lease,
            )
            .await
            {
                Ok((headers, body)) => {
                    status = StatusCode::OK;
                    response_headers = headers;
                    bytes = body;
                }
                Err(error) => {
                    let (failure, headers) = *error;
                    let mut event = failed_usage(&route, u16::from(budget.dispatches()), failure);
                    let state = settle_attempt_failure(
                        &runtime,
                        &lease,
                        &route.source_model,
                        &failure,
                        &headers,
                    );
                    apply_failure_state(&mut event, state);
                    emit_usage(&runtime, event);
                    return finish_request_failure(
                        &runtime,
                        &key,
                        &resolved_model,
                        &[WireApi::Responses],
                        operation,
                        &account_only_exclusions,
                        response_affinity_key.as_deref(),
                        failure,
                        None,
                        selected_error_origin,
                        &request_id,
                    );
                }
            }
        }
        let attempt = u16::from(budget.dispatches());
        if !status.is_success() {
            let rejection_state = settle_attempt_failure(
                &runtime,
                &lease,
                &route.source_model,
                &AttemptFailure::status_with_body(status, Some(&bytes)),
                &response_headers,
            );
            if !legacy_call_id_repair_attempted
                && responses_tool_call_links_rejected(&bytes)
                && repair_legacy_responses_call_ids(&mut request)
            {
                legacy_call_id_repair_attempted = true;

                tried.remove(&route.candidate_id);
                lease.allow_rotation_repair();
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

                tried.remove(&route.candidate_id);
                lease.allow_rotation_repair();
                continue;
            }
            if !custom_tool_item_id_repair_attempted
                && responses_custom_tool_item_id_requires_ctc_prefix(&bytes)
                && repair_custom_tool_item_ids(&mut request)
            {
                custom_tool_item_id_repair_attempted = true;

                tried.remove(&route.candidate_id);
                lease.allow_rotation_repair();
                continue;
            }
            if !message_item_id_repair_attempted
                && responses_message_item_id_requires_msg_prefix(&bytes)
                && remove_item_prefixed_message_ids(&mut request)
            {
                message_item_id_repair_attempted = true;

                tried.remove(&route.candidate_id);
                lease.allow_rotation_repair();
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
            let upstream_error =
                crate::usage::UpstreamErrorDetails::from_body(Some(status.as_u16()), &bytes);
            event.upstream_error = Some(upstream_error.clone());
            if endpoint == AccountEndpoint::Wake
                && is_deferred_tool_search_compatibility_error(status, &upstream_error)
                && tool_policy.prepare_deferred_fallback()
            {
                // Retry once with the original catalog. This is still before
                // any client output because account responses are collected
                // before this status branch.
                event.tool_use.policy_fallback = true;
                emit_usage(&runtime, event);
                tried.remove(&route.candidate_id);
                lease.allow_rotation_repair();
                last_failure = Some(failure);
                last_failure_origin = selected_error_origin;
                continue;
            }
            let stream = request
                .get("stream")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !stale_tool_history_recovered
                && has_previous_response_id
                && responses_tool_call_is_missing_output(&bytes)
                && replay_and_prune_stale_tool_history(
                    &runtime,
                    &key.id,
                    &mut request,
                    &resolved_model,
                    now_ms(),
                    stream,
                    &bytes,
                )
            {
                stale_tool_history_recovered = true;
                response_affinity_key = None;
                requires_affinity_owner = false;
                has_unpaired_tool_output = false;
                tried.remove(&route.candidate_id);
                lease.allow_rotation_repair();
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
                    lease.allow_rotation_repair();
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
                let state = rejection_state.clone();
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
            if route.account_id.is_some() {
                relay_account_response_header(&client_headers, &response_headers, &mut response);
            }
            return response;
        }

        let client_stream = request.get("stream").and_then(Value::as_bool) == Some(true);
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
        let client_bytes = if basis_points_route {
            match super::basis_points::translate_response(&bytes, &request) {
                Ok(bytes) => bytes,
                Err(error) => {
                    event.success = false;
                    event.http_status = StatusCode::BAD_GATEWAY.as_u16();
                    event.error_category = Some(error.code().to_string());
                    emit_usage(&runtime, event);
                    lease.settle_rotation_terminal(now_ms());
                    return super::request::adapter_error_response(error);
                }
            }
        } else {
            bytes
        };
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
        if let Ok(response) = serde_json::from_slice::<Value>(&client_bytes) {
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
            response_id_from_bytes(&client_bytes).as_deref(),
            &route.candidate_id,
            now_ms(),
        );
        emit_usage(&runtime, event);
        lease.settle_rotation_success(now_ms());
        if basis_points_route && client_stream {
            let stream_body = match super::basis_points::synthetic_stream(&client_bytes) {
                Ok(stream_body) => stream_body,
                Err(error) => {
                    lease.settle_rotation_terminal(now_ms());
                    return super::request::adapter_error_response(error);
                }
            };
            let mut response =
                proxy_sse_response(status, &response_headers, Body::from(stream_body));
            relay_account_response_header(&client_headers, &response_headers, &mut response);
            return response;
        }
        let mut response = proxy_response(status, &response_headers, Body::from(client_bytes));
        if route.account_id.is_some() {
            relay_account_response_header(&client_headers, &response_headers, &mut response);
        }
        return response;
    }

    if let Some(error) = crate::gateway::errors::admission_error(&budget) {
        return error;
    }
    if let Some(error) = last_adapter_error {
        return adapter_error_response(error);
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
        operation,
        &account_only_exclusions,
        response_affinity_key.as_deref(),
        failure,
        last_preserved_upstream_error.as_ref(),
        last_failure_origin,
        &request_id,
    )
}

fn request_has_previous_response_id(request: &Value) -> bool {
    request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
}

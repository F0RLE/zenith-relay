use super::super::super::continuation::{
    clear_materialized_continuation, RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
    RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
};
use super::super::super::errors::{
    api_error, apply_failure_state, is_deactivated_workspace, preserved_upstream_error,
    previous_response_not_found, prompt_cache_write_rejected, recoverable_response_affinity_miss,
    recoverable_response_model_switch, retryable_failure, settle_attempt_failure, AttemptFailure,
    PreservedUpstreamError,
};
use super::super::super::now_ms;
use super::super::super::response::{emit_usage, proxy_error_response, usage_event, UsageAttempt};
use super::super::super::turn_state::relay_account_response_header;
use super::super::request::recover_stale_tool_history;
use super::super::AttemptRepairs;
use super::super::{
    repair_once, repair_responses_item_prefixes, reset_materialized_continuation,
    ContinuationReset, ResponsesItemPrefixRepairs,
};
use crate::error_codes;
use crate::gateway::request::AccountEndpoint;
use crate::runtime::{AuthenticatedKey, CandidateLease, ExecutorRoute};
use crate::usage::{ReasoningEffortDiagnostics, ToolUseDiagnostics};
use crate::{ErrorOrigin, GatewayRuntime};
use axum::body::Body;
use axum::http::{HeaderMap, Response, StatusCode};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Instant;

pub(super) enum AccountStatusFailure {
    Continue,
    Respond(Response<Body>),
}

pub(super) struct AccountStatusFailureInput<'a> {
    pub(super) status: StatusCode,
    pub(super) bytes: Vec<u8>,
    pub(super) endpoint: AccountEndpoint,
    pub(super) response_headers: &'a HeaderMap,
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) lease: &'a CandidateLease,
    pub(super) route: &'a ExecutorRoute,
    pub(super) attempt: u16,
    pub(super) request_id: &'a str,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) reasoning_effort: &'a ReasoningEffortDiagnostics,
    pub(super) requested_model: &'a str,
    pub(super) tool_use: &'a ToolUseDiagnostics,
    pub(super) started: Instant,
    pub(super) request: &'a mut Value,
    pub(super) resolved_model: &'a str,
    pub(super) client_headers: &'a HeaderMap,
    pub(super) selected_error_origin: ErrorOrigin,
    pub(super) response_affinity_hit: bool,
    pub(super) has_previous_response_id: bool,
    pub(super) prompt_affinity_key: &'a Option<String>,
    pub(super) repairs: &'a mut AttemptRepairs,
    pub(super) tried: &'a mut HashSet<String>,
    pub(super) has_unpaired_tool_output: &'a mut bool,
    pub(super) requires_affinity_owner: &'a mut bool,
    pub(super) response_affinity_key: &'a mut Option<String>,
    pub(super) last_failure: &'a mut Option<AttemptFailure>,
    pub(super) last_failure_origin: &'a mut ErrorOrigin,
    pub(super) last_preserved_upstream_error: &'a mut Option<PreservedUpstreamError>,
}

struct ClassifiedAccountFailure {
    failure: AttemptFailure,
    event: crate::usage::UsageEvent,
    cache_write_rejected: bool,
}

/// Classify one unsuccessful account response. Repairs continue the attempt
/// loop; a terminal upstream error leaves it.
pub(super) fn handle_account_status_failure(
    mut input: AccountStatusFailureInput<'_>,
) -> AccountStatusFailure {
    if let Some(step) = repair_account_body(&mut input) {
        return step;
    }
    let mut classified = classify_account_failure(&mut input);
    if let Some(step) = repair_classified_account_failure(&mut input, &mut classified) {
        return step;
    }
    settle_account_failure(input, classified)
}

fn repair_account_body(input: &mut AccountStatusFailureInput<'_>) -> Option<AccountStatusFailure> {
    {
        let request = &mut *input.request;
        let tried = &mut *input.tried;
        let candidate_id = input.route.candidate_id.as_str();
        if repair_once(
            &mut input.repairs.legacy_call_id,
            crate::gateway::errors::responses_tool_call_links_rejected(&input.bytes),
            tried,
            candidate_id,
            input.lease,
            || repair_legacy_call_ids(request),
        ) {
            *input.has_unpaired_tool_output =
                !super::super::super::request::unpaired_tool_output_ids(request).is_empty();
            *input.requires_affinity_owner =
                super::request_has_previous_response_id(request) || *input.has_unpaired_tool_output;
            // The provider rejected the body before doing the work. Close that
            // attempt as not sent so the repaired request can use the same
            // account without a cooldown or an unknown cancellation.
            input.lease.settle_rotation_repair(now_ms());
            return Some(AccountStatusFailure::Continue);
        }
    }
    let request = &mut *input.request;
    let tried = &mut *input.tried;
    let candidate_id = input.route.candidate_id.as_str();
    if repair_responses_item_prefixes(
        request,
        &input.bytes,
        true,
        &mut ResponsesItemPrefixRepairs {
            function_ids: &mut input.repairs.function_item_id,
            custom_tool_ids: &mut input.repairs.custom_tool_item_id,
            message_ids: &mut input.repairs.message_item_id,
        },
        tried,
        candidate_id,
        input.lease,
    ) {
        input.lease.settle_rotation_repair(now_ms());
        return Some(AccountStatusFailure::Continue);
    }
    None
}

fn classify_account_failure(input: &mut AccountStatusFailureInput<'_>) -> ClassifiedAccountFailure {
    let mut failure = AttemptFailure::status_with_body(input.status, Some(&input.bytes));
    super::super::super::errors::apply_degraded_route_policy(input.runtime, &mut failure);
    if input.status == StatusCode::PAYMENT_REQUIRED
        && input.route.account_id.is_some()
        && is_deactivated_workspace(&input.bytes)
    {
        input
            .runtime
            .trip_chatgpt_team_breaker(&input.route.candidate_id, now_ms());
    }
    *input.last_preserved_upstream_error = preserved_upstream_error(&failure, &input.bytes);
    let upstream_error =
        crate::usage::UpstreamErrorDetails::from_body(Some(input.status.as_u16()), &input.bytes);
    let mut event = usage_event(
        UsageAttempt {
            request_id: input.request_id,
            attempt: input.attempt,
            local_key_id: &input.key.id,
            route: input.route,
            reasoning_effort: Some(input.reasoning_effort),
            requested_model: input.requested_model,
            tool_use: input.tool_use.clone(),
        },
        false,
        input.status.as_u16(),
        Some(failure.category.to_string()),
        input.started.elapsed().as_millis() as u64,
    );
    event.upstream_error = Some(upstream_error.clone());
    ClassifiedAccountFailure {
        cache_write_rejected: prompt_cache_write_rejected(&input.bytes),
        failure,
        event,
    }
}

fn repair_classified_account_failure(
    input: &mut AccountStatusFailureInput<'_>,
    classified: &mut ClassifiedAccountFailure,
) -> Option<AccountStatusFailure> {
    {
        let request = &mut *input.request;
        let tried = &mut *input.tried;
        let candidate_id = input.route.candidate_id.as_str();
        let supports_responses_history = matches!(
            input.endpoint,
            AccountEndpoint::Compact | AccountEndpoint::Wake
        );
        if supports_responses_history
            && repair_once(
                &mut input.repairs.encrypted_context,
                classified.failure.category == error_codes::UPSTREAM_ENCRYPTED_CONTENT_INVALID,
                tried,
                candidate_id,
                input.lease,
                || super::drop_rejected_encrypted_context(request),
            )
        {
            emit_usage(input.runtime, classified.event.clone());
            *input.last_failure = Some(classified.failure);
            *input.last_failure_origin = input.selected_error_origin;
            input.lease.settle_rotation_repair(now_ms());
            return Some(AccountStatusFailure::Continue);
        }
    }
    if input.has_previous_response_id
        && recover_stale_tool_history(
            input.runtime,
            &input.key.id,
            input.request,
            input.resolved_model,
            &input.bytes,
            &mut input.repairs.stale_tool_history,
        )
    {
        clear_materialized_continuation(
            input.response_affinity_key,
            input.requires_affinity_owner,
            input.has_unpaired_tool_output,
        );
        input.tried.remove(&input.route.candidate_id);
        input.lease.settle_rotation_repair(now_ms());
        emit_usage(input.runtime, classified.event.clone());
        *input.last_failure = Some(classified.failure);
        *input.last_failure_origin = input.selected_error_origin;
        return Some(AccountStatusFailure::Continue);
    }
    if !input.repairs.model_switch_reset
        && reset_materialized_continuation(
            &mut ContinuationReset {
                attempted: &mut input.repairs.model_switch_reset,
                response_affinity_key: input.response_affinity_key,
                requires_affinity_owner: input.requires_affinity_owner,
            },
            recoverable_response_model_switch(
                input.status,
                classified.failure.category,
                input.has_previous_response_id,
                *input.has_unpaired_tool_output,
                &input.bytes,
            ),
            input.runtime,
            &input.key.id,
            input.request,
            input.resolved_model,
        )
    {
        emit_usage(input.runtime, classified.event.clone());
        *input.last_failure = Some(classified.failure);
        *input.last_failure_origin = input.selected_error_origin;
        input.lease.settle_rotation_repair(now_ms());
        return Some(AccountStatusFailure::Continue);
    }
    None
}

fn settle_account_failure(
    input: AccountStatusFailureInput<'_>,
    classified: ClassifiedAccountFailure,
) -> AccountStatusFailure {
    let AccountStatusFailureInput {
        status,
        bytes,
        response_headers,
        runtime,
        lease,
        route,
        request_id,
        key,
        request,
        resolved_model,
        client_headers,
        selected_error_origin,
        response_affinity_hit,
        has_previous_response_id,
        prompt_affinity_key,
        repairs,
        tried,
        requires_affinity_owner,
        response_affinity_key,
        last_failure,
        last_failure_origin,
        ..
    } = input;
    let failure = classified.failure;
    let mut event = classified.event;
    let affinity_miss = recoverable_response_affinity_miss(
        status,
        has_previous_response_id,
        response_affinity_hit,
        previous_response_not_found(&bytes),
    );
    if affinity_miss {
        event.error_category = Some(error_codes::RESPONSE_AFFINITY_MISS.to_string());
        emit_usage(runtime, event);
        if reset_materialized_continuation(
            &mut ContinuationReset {
                attempted: &mut repairs.model_switch_reset,
                response_affinity_key,
                requires_affinity_owner,
            },
            true,
            runtime,
            &key.id,
            request,
            resolved_model,
        ) {
            tried.remove(&route.candidate_id);
            lease.settle_rotation_repair(now_ms());
            *last_failure = Some(failure);
            *last_failure_origin = selected_error_origin;
            return AccountStatusFailure::Continue;
        }
        settle_attempt_failure(
            runtime,
            lease,
            &route.source_model,
            &failure,
            response_headers,
        );
        return AccountStatusFailure::Respond(api_error(
            StatusCode::CONFLICT,
            RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
            RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
        ));
    }
    let rejection_state = settle_attempt_failure(
        runtime,
        lease,
        &route.source_model,
        &failure,
        response_headers,
    );
    if classified.cache_write_rejected {
        runtime.invalidate_prompt_affinity(prompt_affinity_key.as_deref());
    }
    if classified.cache_write_rejected
        || retryable_failure(status, failure.category, has_previous_response_id)
    {
        if response_affinity_hit && !*requires_affinity_owner {
            *response_affinity_key = None;
        }
        let state = rejection_state.clone();
        apply_failure_state(&mut event, state);
        emit_usage(runtime, event);
        *last_failure = Some(failure);
        *last_failure_origin = selected_error_origin;
        return AccountStatusFailure::Continue;
    }
    emit_usage(runtime, event);
    let origin = selected_error_origin.for_category(failure.category);
    let mut response = proxy_error_response(
        status,
        response_headers,
        &bytes,
        origin,
        failure.category,
        Some(request_id),
    );
    if route.account_id.is_some() {
        relay_account_response_header(client_headers, response_headers, &mut response);
    }
    AccountStatusFailure::Respond(response)
}

fn repair_legacy_call_ids(request: &mut Value) -> bool {
    super::super::super::request::repair_legacy_responses_call_ids(request)
}

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
    bind_encrypted_context_repair_owner, repair_once, repair_responses_item_prefixes,
    reset_materialized_continuation, reset_materialized_continuation_for_owner_retry,
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
    mut account_failure_input: AccountStatusFailureInput<'_>,
) -> AccountStatusFailure {
    if let Some(step) = repair_account_body(&mut account_failure_input) {
        return step;
    }
    let mut classified = classify_account_failure(&mut account_failure_input);
    if let Some(step) =
        repair_classified_account_failure(&mut account_failure_input, &mut classified)
    {
        return step;
    }
    settle_account_failure(account_failure_input, classified)
}

fn repair_account_body(
    account_failure_input: &mut AccountStatusFailureInput<'_>,
) -> Option<AccountStatusFailure> {
    {
        let request_body = &mut *account_failure_input.request;
        let tried = &mut *account_failure_input.tried;
        let candidate_id = account_failure_input.route.candidate_id.as_str();
        if repair_once(
            &mut account_failure_input.repairs.legacy_call_id,
            crate::gateway::errors::responses_tool_call_links_rejected(
                &account_failure_input.bytes,
            ),
            tried,
            candidate_id,
            account_failure_input.lease,
            || repair_legacy_call_ids(request_body),
        ) {
            *account_failure_input.has_unpaired_tool_output =
                !super::super::super::request::unpaired_tool_output_ids(request_body).is_empty();
            *account_failure_input.requires_affinity_owner =
                super::request_has_previous_response_id(request_body)
                    || *account_failure_input.has_unpaired_tool_output;
            // The provider rejected the body before doing the work. Close that
            // attempt as not sent so the repaired request can use the same
            // account without a cooldown or an unknown cancellation.
            account_failure_input.lease.settle_rotation_repair(now_ms());
            return Some(AccountStatusFailure::Continue);
        }
    }
    let request_body = &mut *account_failure_input.request;
    let tried = &mut *account_failure_input.tried;
    let candidate_id = account_failure_input.route.candidate_id.as_str();
    if repair_responses_item_prefixes(
        request_body,
        &account_failure_input.bytes,
        true,
        &mut ResponsesItemPrefixRepairs {
            function_ids: &mut account_failure_input.repairs.function_item_id,
            custom_tool_ids: &mut account_failure_input.repairs.custom_tool_item_id,
            message_ids: &mut account_failure_input.repairs.message_item_id,
        },
        tried,
        candidate_id,
        account_failure_input.lease,
    ) {
        account_failure_input.lease.settle_rotation_repair(now_ms());
        return Some(AccountStatusFailure::Continue);
    }
    None
}

fn classify_account_failure(
    account_failure_input: &mut AccountStatusFailureInput<'_>,
) -> ClassifiedAccountFailure {
    let mut failure = AttemptFailure::status_with_body(
        account_failure_input.status,
        Some(&account_failure_input.bytes),
    );
    super::super::super::errors::apply_degraded_route_policy(
        account_failure_input.runtime,
        &mut failure,
    );
    if account_failure_input.status == StatusCode::PAYMENT_REQUIRED
        && account_failure_input.route.account_id.is_some()
        && is_deactivated_workspace(&account_failure_input.bytes)
    {
        account_failure_input
            .runtime
            .trip_chatgpt_team_breaker(&account_failure_input.route.candidate_id, now_ms());
    }
    *account_failure_input.last_preserved_upstream_error =
        preserved_upstream_error(&failure, &account_failure_input.bytes);
    let upstream_error = crate::usage::UpstreamErrorDetails::from_response_body(
        Some(account_failure_input.status.as_u16()),
        &account_failure_input.bytes,
    );
    let mut event = usage_event(
        UsageAttempt {
            request_id: account_failure_input.request_id,
            attempt: account_failure_input.attempt,
            local_key_id: &account_failure_input.key.id,
            route: account_failure_input.route,
            reasoning_effort: Some(account_failure_input.reasoning_effort),
            requested_model: account_failure_input.requested_model,
            tool_use: account_failure_input.tool_use.clone(),
        },
        false,
        account_failure_input.status.as_u16(),
        Some(failure.category.to_string()),
        account_failure_input.started.elapsed().as_millis() as u64,
    );
    event.upstream_error = Some(upstream_error.clone());
    ClassifiedAccountFailure {
        cache_write_rejected: prompt_cache_write_rejected(&account_failure_input.bytes),
        failure,
        event,
    }
}

fn repair_classified_account_failure(
    account_failure_input: &mut AccountStatusFailureInput<'_>,
    classified: &mut ClassifiedAccountFailure,
) -> Option<AccountStatusFailure> {
    {
        let request_body = &mut *account_failure_input.request;
        let tried = &mut *account_failure_input.tried;
        let candidate_id = account_failure_input.route.candidate_id.as_str();
        let supports_responses_history = matches!(
            account_failure_input.endpoint,
            AccountEndpoint::Compact | AccountEndpoint::Wake
        );
        if supports_responses_history
            && repair_once(
                &mut account_failure_input.repairs.encrypted_context,
                classified.failure.category == error_codes::UPSTREAM_ENCRYPTED_CONTENT_INVALID,
                tried,
                candidate_id,
                account_failure_input.lease,
                || super::drop_rejected_encrypted_context(request_body),
            )
        {
            bind_encrypted_context_repair_owner(
                account_failure_input.repairs,
                account_failure_input.response_affinity_key,
                account_failure_input.requires_affinity_owner,
                account_failure_input.runtime,
                account_failure_input.request_id,
                candidate_id,
            );
            emit_usage(account_failure_input.runtime, classified.event.clone());
            *account_failure_input.last_failure = Some(classified.failure);
            *account_failure_input.last_failure_origin =
                account_failure_input.selected_error_origin;
            account_failure_input.lease.settle_rotation_repair(now_ms());
            return Some(AccountStatusFailure::Continue);
        }
    }
    if account_failure_input.has_previous_response_id
        && recover_stale_tool_history(
            account_failure_input.runtime,
            &account_failure_input.key.id,
            account_failure_input.request,
            account_failure_input.resolved_model,
            &account_failure_input.bytes,
            &mut account_failure_input.repairs.stale_tool_history,
        )
    {
        clear_materialized_continuation(
            account_failure_input.response_affinity_key,
            account_failure_input.requires_affinity_owner,
            account_failure_input.has_unpaired_tool_output,
        );
        account_failure_input
            .tried
            .remove(&account_failure_input.route.candidate_id);
        account_failure_input.lease.settle_rotation_repair(now_ms());
        emit_usage(account_failure_input.runtime, classified.event.clone());
        *account_failure_input.last_failure = Some(classified.failure);
        *account_failure_input.last_failure_origin = account_failure_input.selected_error_origin;
        return Some(AccountStatusFailure::Continue);
    }
    if !account_failure_input.repairs.model_switch_reset
        && reset_materialized_continuation(
            &mut ContinuationReset {
                attempted: &mut account_failure_input.repairs.model_switch_reset,
                response_affinity_key: account_failure_input.response_affinity_key,
                requires_affinity_owner: account_failure_input.requires_affinity_owner,
            },
            recoverable_response_model_switch(
                account_failure_input.status,
                classified.failure.category,
                account_failure_input.has_previous_response_id,
                *account_failure_input.has_unpaired_tool_output,
                &account_failure_input.bytes,
            ),
            account_failure_input.runtime,
            &account_failure_input.key.id,
            account_failure_input.request,
            account_failure_input.resolved_model,
        )
    {
        emit_usage(account_failure_input.runtime, classified.event.clone());
        *account_failure_input.last_failure = Some(classified.failure);
        *account_failure_input.last_failure_origin = account_failure_input.selected_error_origin;
        account_failure_input.lease.settle_rotation_repair(now_ms());
        return Some(AccountStatusFailure::Continue);
    }
    None
}

fn settle_account_failure(
    account_failure_input: AccountStatusFailureInput<'_>,
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
    } = account_failure_input;
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
        if reset_materialized_continuation_for_owner_retry(
            &mut repairs.model_switch_reset,
            response_affinity_key,
            requires_affinity_owner,
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
        let rejection_state = rejection_state.clone();
        apply_failure_state(&mut event, rejection_state);
        emit_usage(runtime, event);
        *last_failure = Some(failure);
        *last_failure_origin = selected_error_origin;
        return AccountStatusFailure::Continue;
    }
    emit_usage(runtime, event);
    let origin = selected_error_origin.for_category(failure.category);
    let mut error_response = proxy_error_response(
        status,
        response_headers,
        &bytes,
        origin,
        failure.category,
        Some(request_id),
    );
    if route.account_id.is_some() {
        relay_account_response_header(client_headers, response_headers, &mut error_response);
    }
    AccountStatusFailure::Respond(error_response)
}

fn repair_legacy_call_ids(request: &mut Value) -> bool {
    super::super::super::request::repair_legacy_responses_call_ids(request)
}

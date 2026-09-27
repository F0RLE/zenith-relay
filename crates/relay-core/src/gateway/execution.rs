mod account;
mod basis_points;
mod client;
mod compatibility;
mod request;

pub(super) use account::{execute_account_endpoint, AccountExecution};
pub(super) use client::RoutedRequestIdentity;
pub(super) use client::{execute_client_request, execute_gemini_client_request};

use super::errors::{
    api_error_with_origin, api_error_with_origin_and_category, cooldown_error, AttemptFailure,
    PreservedUpstreamError,
};
use super::errors::{
    responses_custom_tool_item_id_requires_ctc_prefix,
    responses_function_item_id_requires_fc_prefix, responses_message_item_id_requires_msg_prefix,
};
use super::now_ms;
use super::request::response_tool_call_ids;
use super::response::response_id_from_bytes;
use crate::protocol::{
    remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids,
    repair_custom_tool_item_ids,
};
use crate::runtime::{AuthenticatedKey, GatewayRuntime};
use crate::scheduler::rotation::RotationOperation;
use crate::ErrorOrigin;
use axum::body::Body;
use axum::http::{Response, StatusCode};
use serde_json::Value;
use std::collections::HashSet;

/// Records the one allowed model-switch reset and drops the opaque continuation binding.
///
/// Usage, lease, and retry steps stay with the caller: ordinary requests and
/// account-only execution do not share those aftermaths.
pub(super) fn mark_model_switch_reset(
    attempted: &mut bool,
    response_affinity_key: &mut Option<String>,
    requires_affinity_owner: &mut bool,
) {
    *attempted = true;
    *response_affinity_key = None;
    *requires_affinity_owner = false;
}

/// Accepts one pre-output request repair and lets the same candidate be selected again.
///
/// The predicate and mutation stay with the caller so a failed repair does not
/// consume the one-shot flag. Ordinary requests still settle the lease
/// themselves; account-only execution does not.
pub(super) fn repair_once(
    attempted: &mut bool,
    eligible: bool,
    tried: &mut HashSet<String>,
    candidate_id: &str,
    lease: &crate::runtime::CandidateLease,
    repair: impl FnOnce() -> bool,
) -> bool {
    if *attempted || !eligible || !repair() {
        return false;
    }
    *attempted = true;
    tried.remove(candidate_id);
    lease.allow_rotation_repair();
    true
}

/// One-shot flags for the three Responses item-id repairs.
pub(super) struct ResponsesItemPrefixRepairs<'a> {
    pub(super) function_ids: &'a mut bool,
    pub(super) custom_tool_ids: &'a mut bool,
    pub(super) message_ids: &'a mut bool,
}

/// Repairs `fc_`, `ctc_`, and `msg_` item ids in that order, at most one per call.
///
/// `enabled` is false for an adapted ordinary request. Account execution and
/// WebSocket are already native Responses, so they pass true. Lease settlement
/// and failure bookkeeping stay with the caller: an ordinary request settles
/// the lease, an account does not, and WebSocket clears its last failure.
pub(super) fn repair_responses_item_prefixes(
    request: &mut Value,
    body: &[u8],
    enabled: bool,
    repairs: &mut ResponsesItemPrefixRepairs<'_>,
    tried: &mut HashSet<String>,
    candidate_id: &str,
    lease: &crate::runtime::CandidateLease,
) -> bool {
    enabled
        && (repair_once(
            repairs.function_ids,
            responses_function_item_id_requires_fc_prefix(body),
            tried,
            candidate_id,
            lease,
            || repair_call_prefixed_function_item_ids(request),
        ) || repair_once(
            repairs.custom_tool_ids,
            responses_custom_tool_item_id_requires_ctc_prefix(body),
            tried,
            candidate_id,
            lease,
            || repair_custom_tool_item_ids(request),
        ) || repair_once(
            repairs.message_ids,
            responses_message_item_id_requires_msg_prefix(body),
            tried,
            candidate_id,
            lease,
            || remove_item_prefixed_message_ids(request),
        ))
}

/// Builds the final response after all pre-output route attempts are exhausted.
/// Account and ordinary client execution use the same cooldown and preserved
/// provider-error policy; keeping it here prevents the two retry loops from
/// drifting apart.
#[allow(clippy::too_many_arguments)]
pub(super) fn finish_request_failure(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    resolved_model: &str,
    protocols: &[crate::WireApi],
    operation: RotationOperation,
    exclusions: &HashSet<String>,
    response_affinity_key: Option<&str>,
    failure: AttemptFailure,
    preserved: Option<&PreservedUpstreamError>,
    failure_origin: ErrorOrigin,
    request_id: &str,
) -> Response<Body> {
    if failure.status == StatusCode::TOO_MANY_REQUESTS {
        if let Some((retry_at, reason)) = runtime.all_applicable_cooldown(
            key,
            resolved_model,
            protocols,
            exclusions,
            response_affinity_key,
            now_ms(),
            operation,
        ) {
            return cooldown_error(
                retry_at,
                Some(&failure),
                reason == crate::scheduler::CooldownReason::RateLimit,
            );
        }
    }
    attempt_error_response(failure, preserved, failure_origin, request_id)
}

/// Keep the safe upstream error only when it belongs to this exact failure.
/// Terminal JSON, stream bootstrap and exhausted retries share this policy.
pub(super) fn attempt_error_response(
    failure: AttemptFailure,
    preserved: Option<&PreservedUpstreamError>,
    failure_origin: ErrorOrigin,
    request_id: &str,
) -> Response<Body> {
    if let Some(preserved) = preserved.filter(|preserved| {
        preserved.status == failure.status && preserved.category == failure.category
    }) {
        return api_error_with_origin_and_category(
            preserved.status,
            &preserved.message,
            &preserved.code,
            preserved.category,
            failure_origin,
            Some(request_id),
        );
    }
    api_error_with_origin(
        failure.status,
        failure.message,
        failure.category,
        failure_origin,
        Some(request_id),
    )
}

pub(super) struct CandidateRetryContext<'a> {
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) resolved_model: &'a str,
    pub(super) protocols: &'a [crate::WireApi],
    pub(super) operation: RotationOperation,
    pub(super) exclusions: &'a HashSet<String>,
}

/// Await the sole scheduler's next actionable event without a reservation.
/// Attempt counts and the monotonic window belong to the incoming request,
/// not a second recovery state machine created by each transport driver.
pub(super) async fn wait_for_recovery(
    budget: &crate::scheduler::rotation::SharedRequestBudget,
    context: &CandidateRetryContext<'_>,
    tried: &mut HashSet<String>,
    response_affinity_key: Option<&str>,
) -> bool {
    if !budget.can_dispatch() {
        return false;
    }
    // Only an actionable recovery opens the retry window. A plain no-route
    // result must not start it before the first safe rejection/wait.
    if context
        .runtime
        .recovery_retry_at(
            context.key,
            context.resolved_model,
            context.protocols,
            context.exclusions,
            response_affinity_key,
            now_ms(),
            context.operation,
        )
        .is_none()
    {
        return false;
    }
    let deadline = budget.retry_wait_deadline(context.runtime.route_recovery_window_ms());
    if !context
        .runtime
        .wait_for_recovery_event(
            context.key,
            context.resolved_model,
            context.protocols,
            context.exclusions,
            response_affinity_key,
            context.operation,
            budget,
            Some(deadline),
            false,
        )
        .await
    {
        return false;
    }
    tried.clone_from(context.exclusions);
    budget.begin_recovery_pass();
    true
}

pub(super) async fn wait_for_candidate_retry(
    budget: &crate::scheduler::rotation::SharedRequestBudget,
    context: &CandidateRetryContext<'_>,
    tried: &mut HashSet<String>,
    response_affinity_key: Option<&str>,
) -> bool {
    tried.clone_from(context.exclusions);
    let ready = context
        .runtime
        .wait_for_recovery_event(
            context.key,
            context.resolved_model,
            context.protocols,
            context.exclusions,
            response_affinity_key,
            context.operation,
            budget,
            None,
            true,
        )
        .await;
    if ready {
        budget.begin_recovery_pass();
    }
    ready
}

/// Records affinity for one completed Responses body.
///
/// The body is parsed once. Replay capture is optional: a native passthrough
/// keeps the materialized turn, and an adapted body still records tool-call
/// and response affinity. Account execution always captures because that route
/// is already native Responses. Usage emission and lease settlement stay with
/// the caller; those two paths close the lease in opposite orders.
pub(super) fn bind_responses_turn(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    candidate_id: &str,
    request: &Value,
    source_model: &str,
    body: &[u8],
    capture_replay: bool,
) {
    if let Ok(response) = serde_json::from_slice::<Value>(body) {
        if capture_replay {
            runtime.capture_native_responses_replay(
                local_key_id,
                candidate_id,
                request,
                source_model,
                &response,
                now_ms(),
            );
        }
        for call_id in response_tool_call_ids(&response) {
            runtime.bind_tool_call_affinity(local_key_id, &call_id, candidate_id, now_ms());
        }
    }
    runtime.bind_response_affinity(
        response_id_from_bytes(body).as_deref(),
        candidate_id,
        now_ms(),
    );
}

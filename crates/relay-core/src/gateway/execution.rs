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
use crate::error_codes;
use crate::protocol::{
    remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids,
    repair_custom_tool_item_ids,
};
use crate::runtime::{AuthenticatedKey, GatewayRuntime};
use crate::scheduler::rotation::RotationOperation;
use crate::ErrorOrigin;
use axum::body::Body;
use axum::http::{HeaderValue, Response, StatusCode};
use serde_json::Value;
use std::collections::HashSet;

/// Header shared by ordinary Responses routes and account execution.
///
/// The caller decides when the route is eligible. Ordinary routes pass this
/// only for `WireApi::Responses`; account execution is already on that protocol.
fn responses_lite_header(
    responses_lite: &Option<HeaderValue>,
    automatic_responses_lite: bool,
    runtime: &GatewayRuntime,
    resolved_model: &str,
    account_id: Option<&str>,
) -> Option<HeaderValue> {
    responses_lite.clone().or_else(|| {
        (automatic_responses_lite
            && account_id.is_some_and(|candidate_id| {
                runtime
                    .codex_model_responses_lite_candidates(resolved_model)
                    .iter()
                    .any(|id| id == candidate_id)
            }))
        .then(|| HeaderValue::from_static("true"))
    })
}

/// One-shot request repairs shared by ordinary and account execution.
///
/// Each flag records that its repair already ran for this request. The attempt
/// loop owns the value; failure, selection, and stream steps borrow it.
#[derive(Default)]
pub(super) struct AttemptRepairs {
    pub(super) native_replay: bool,
    pub(super) function_item_id: bool,
    pub(super) custom_tool_item_id: bool,
    pub(super) message_item_id: bool,
    pub(super) legacy_call_id: bool,
    pub(super) model_switch_reset: bool,
    pub(super) stale_tool_history: bool,
    pub(super) quota_yield: bool,
    /// Basis Points rejected ciphertext from another model or account.
    pub(super) encrypted_context: bool,
}

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

/// Drops one opaque continuation after the caller has decided the reset is allowed.
///
/// The one-shot flag stays unchanged when the drop does not apply. Usage, lease,
/// and candidate retry remain with the caller.
fn reset_opaque_continuation(
    attempted: &mut bool,
    eligible: bool,
    drop_previous: impl FnOnce() -> bool,
    response_affinity_key: &mut Option<String>,
    requires_affinity_owner: &mut bool,
) -> bool {
    if *attempted || !eligible || !drop_previous() {
        return false;
    }
    mark_model_switch_reset(attempted, response_affinity_key, requires_affinity_owner);
    true
}

/// The one-shot state for dropping an opaque `previous_response_id`.
pub(super) struct ContinuationReset<'a> {
    pub(super) attempted: &'a mut bool,
    pub(super) response_affinity_key: &'a mut Option<String>,
    pub(super) requires_affinity_owner: &'a mut bool,
}

/// Restores a saved turn and forgets its opaque response id.
///
/// The caller decides whether that reset is allowed. Usage, lease, and candidate
/// retry stay with the caller. A failed restore does not consume the one-shot flag.
pub(super) fn reset_materialized_continuation(
    reset: &mut ContinuationReset<'_>,
    eligible: bool,
    runtime: &GatewayRuntime,
    local_key_id: &str,
    request: &mut Value,
    resolved_model: &str,
) -> bool {
    let now = now_ms();
    reset_opaque_continuation(
        reset.attempted,
        eligible,
        || {
            super::continuation::drop_materialized_previous_response_id(
                runtime,
                local_key_id,
                request,
                resolved_model,
                now,
            )
        },
        reset.response_affinity_key,
        reset.requires_affinity_owner,
    )
}

/// Accepts one pre-output request repair and lets the same candidate be selected again.
///
/// The predicate and mutation stay with the caller so a failed repair does not
/// consume the one-shot flag. A successful repair only permits the same member
/// again. The caller closes a started lease with `settle_rotation_repair`
/// before continuing; dropping it records an unknown outcome and stops retry.
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
/// WebSocket are already native Responses, so they pass true. The caller still
/// settles the lease: a proven repair is not a route failure, so it must be
/// closed with `settle_rotation_repair` before any rejection settlement.
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
pub(super) struct RequestFailureInput<'a> {
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) resolved_model: &'a str,
    pub(super) protocols: &'a [crate::WireApi],
    pub(super) operation: RotationOperation,
    pub(super) exclusions: &'a HashSet<String>,
    pub(super) response_affinity_key: Option<&'a str>,
    pub(super) failure: AttemptFailure,
    pub(super) preserved: Option<&'a PreservedUpstreamError>,
    pub(super) failure_origin: ErrorOrigin,
    pub(super) request_id: &'a str,
}

pub(super) fn finish_request_failure(input: RequestFailureInput<'_>) -> Response<Body> {
    let RequestFailureInput {
        runtime,
        key,
        resolved_model,
        protocols,
        operation,
        exclusions,
        response_affinity_key,
        failure,
        preserved,
        failure_origin,
        request_id,
    } = input;
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
    let code = match failure.category {
        error_codes::UPSTREAM_BODY | error_codes::UPSTREAM_BODY_TOO_LARGE => {
            error_codes::UPSTREAM_ERROR
        }
        _ => {
            return api_error_with_origin(
                failure.status,
                failure.message,
                failure.category,
                failure_origin,
                Some(request_id),
            );
        }
    };
    api_error_with_origin_and_category(
        failure.status,
        failure.message,
        code,
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

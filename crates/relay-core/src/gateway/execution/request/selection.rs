use super::super::state::AttemptState;
use super::prelude::*;
use super::recovery::{
    replay_native_affinity_continuation, should_wait_for_candidate_availability,
};

pub(super) enum SelectionMiss {
    Continue,
    Stop { retry_window_expired: bool },
    AdapterError(AdapterError),
    Respond(Response<Body>),
}

pub(super) struct SelectionMissInput<'a> {
    pub(super) budget: &'a SharedRequestBudget,
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) resolved_model: &'a str,
    pub(super) client_wire_api: WireApi,
    pub(super) stream: bool,
    pub(super) request_json: &'a mut Value,
    pub(super) state: &'a mut AttemptState,
    pub(super) allow_previous_response_reset: bool,
    pub(super) has_previous_response_id: bool,
    pub(super) retry_context: &'a CandidateRetryContext<'a>,
    pub(super) retry_until_available: bool,
}

/// No reserved candidate is available for this attempt. Recover a pinned
/// continuation, wait for a route, or stop with the same response the loop
/// used to return directly.
pub(super) async fn handle_selection_miss(mut input: SelectionMissInput<'_>) -> SelectionMiss {
    if let Some(error) = crate::gateway::errors::admission_error(input.budget) {
        return SelectionMiss::Respond(error);
    }
    if release_unroutable_affinity(&mut input) {
        return SelectionMiss::Continue;
    }
    if reset_owner_without_model(&mut input) {
        return SelectionMiss::Continue;
    }
    if let Some(step) = replay_pinned_continuation(&mut input) {
        return step;
    }
    if reset_owner_outside_scope(&mut input) {
        return SelectionMiss::Continue;
    }
    if let Some(step) = reject_unavailable_pinned_owner(&input) {
        return step;
    }
    if wait_for_route_recovery(&mut input).await {
        return SelectionMiss::Continue;
    }
    if let Some(step) = wait_for_candidate_availability(&mut input).await {
        return step;
    }
    if let Some(step) = first_attempt_cooldown(&input) {
        return step;
    }
    stop_without_candidate(&mut input)
}

fn release_unroutable_affinity(input: &mut SelectionMissInput<'_>) -> bool {
    !input.state.continuation.requires_affinity_owner
        && input.runtime.release_unroutable_response_affinity(
            input.key,
            &mut input.state.continuation.response_affinity_key,
            input.resolved_model,
            candidate_protocols(input.client_wire_api),
            now_ms(),
        )
}

/// Native replay deliberately refuses to materialize a response across
/// model/protocol routes. When the bound owner itself no longer supports this
/// route, retaining its opaque response id would therefore block all eligible
/// new-model candidates before an upstream request is even attempted. Start a
/// fresh safe turn instead, but never infer that from temporary availability.
fn reset_owner_without_model(input: &mut SelectionMissInput<'_>) -> bool {
    if !pinned_responses_reset_allowed(input) {
        return false;
    }
    let owner_supports_model = input
        .state
        .continuation
        .response_affinity_key
        .as_deref()
        .and_then(|affinity_key| {
            input.runtime.response_affinity_owner_supports_model(
                affinity_key,
                input.resolved_model,
                candidate_protocols(input.client_wire_api),
                now_ms(),
            )
        });
    reset_pinned_continuation(input, owner_supports_model == Some(false))
}

/// Pool membership can change between two Codex turns. An opaque
/// previous_response_id remains pinned to its old owner, so normal selection
/// correctly declines to send it to a new provider. If Relay still has the
/// bounded, owner-scoped native replay for that turn, materialize it before
/// trying the replacement pool. This keeps the continuation safe while avoiding
/// a permanent no-candidate failure after an operator rotates API sources.
fn replay_pinned_continuation(input: &mut SelectionMissInput<'_>) -> Option<SelectionMiss> {
    if input.client_wire_api != WireApi::Responses
        || !input.has_previous_response_id
        || !input.state.continuation.requires_affinity_owner
        || input.state.continuation.repairs.native_replay
    {
        return None;
    }
    match replay_native_affinity_continuation(
        input.runtime,
        &input.key.id,
        input.request_json,
        input.state.continuation.response_affinity_key.as_deref(),
        input.resolved_model,
        input.stream,
        &mut input.state.continuation.repairs.native_replay,
    ) {
        Ok(true) => {
            input.state.continuation.clear_materialized();
            Some(SelectionMiss::Continue)
        }
        Ok(false) => None,
        Err(error) => Some(SelectionMiss::AdapterError(error)),
    }
}

/// If the owner still matches the model but is no longer in this key's pool
/// scope, there is no safe upstream route for the opaque response id. Reset it
/// only after giving the bounded native replay above a chance to preserve the
/// conversation.
fn reset_owner_outside_scope(input: &mut SelectionMissInput<'_>) -> bool {
    if !pinned_responses_reset_allowed(input) {
        return false;
    }
    let owner_supports_route = input
        .state
        .continuation
        .response_affinity_key
        .as_deref()
        .and_then(|affinity_key| owner_supports_route(input, affinity_key));
    reset_pinned_continuation(input, owner_supports_route == Some(false))
}

fn reject_unavailable_pinned_owner(input: &SelectionMissInput<'_>) -> Option<SelectionMiss> {
    let unavailable = input.state.continuation.requires_affinity_owner
        && input
            .state
            .continuation
            .response_affinity_key
            .as_deref()
            .and_then(|affinity_key| owner_supports_route(input, affinity_key))
            == Some(false);
    if !unavailable {
        return None;
    }
    Some(SelectionMiss::Respond(api_error(
        StatusCode::CONFLICT,
        RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
        RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
    )))
}

async fn wait_for_route_recovery(input: &mut SelectionMissInput<'_>) -> bool {
    wait_for_recovery(
        input.budget,
        input.retry_context,
        &mut input.state.retry.tried,
        input.state.continuation.response_affinity_key.as_deref(),
    )
    .await
}

async fn wait_for_candidate_availability(
    input: &mut SelectionMissInput<'_>,
) -> Option<SelectionMiss> {
    if !should_wait_for_candidate_availability(
        input.retry_until_available,
        &input.state.retry.last_failure,
        !input.state.retry.preparation_exclusions.is_empty()
            || input.state.retry.last_adapter_error.is_some(),
        input.has_previous_response_id,
    ) {
        return None;
    }
    if !wait_for_candidate_retry(
        input.budget,
        input.retry_context,
        &mut input.state.retry.tried,
        input.state.continuation.response_affinity_key.as_deref(),
    )
    .await
    {
        return Some(SelectionMiss::Stop {
            retry_window_expired: true,
        });
    }
    Some(SelectionMiss::Continue)
}

fn first_attempt_cooldown(input: &SelectionMissInput<'_>) -> Option<SelectionMiss> {
    if input.state.retry.attempt != 0 {
        return None;
    }
    let (retry_at, reason) = input.runtime.all_applicable_cooldown(
        input.key,
        input.resolved_model,
        candidate_protocols(input.client_wire_api),
        &input.state.retry.tried,
        input.state.continuation.response_affinity_key.as_deref(),
        now_ms(),
        crate::scheduler::rotation::RotationOperation::Text,
    )?;
    Some(SelectionMiss::Respond(cooldown_error(
        retry_at,
        None,
        reason.is_rate_limit(),
        crate::ErrorOrigin::Relay,
    )))
}

fn stop_without_candidate(input: &mut SelectionMissInput<'_>) -> SelectionMiss {
    if input.state.retry.last_failure.is_none()
        && let Some(error) = input.state.retry.last_adapter_error.take()
    {
        return SelectionMiss::AdapterError(error);
    }
    SelectionMiss::Stop {
        retry_window_expired: input.state.retry.window_expired,
    }
}

fn pinned_responses_reset_allowed(input: &SelectionMissInput<'_>) -> bool {
    input.client_wire_api == WireApi::Responses
        && input.allow_previous_response_reset
        && input.has_previous_response_id
        && input.state.continuation.requires_affinity_owner
        && !input.state.continuation.has_unpaired_tool_output
        && !input.state.continuation.repairs.model_switch_reset
}

fn owner_supports_route(input: &SelectionMissInput<'_>, affinity_key: &str) -> Option<bool> {
    input.runtime.response_affinity_owner_supports_route(
        input.key,
        affinity_key,
        input.resolved_model,
        candidate_protocols(input.client_wire_api),
        now_ms(),
    )
}

fn reset_pinned_continuation(input: &mut SelectionMissInput<'_>, eligible: bool) -> bool {
    reset_materialized_continuation(
        &mut ContinuationReset {
            attempted: &mut input.state.continuation.repairs.model_switch_reset,
            response_affinity_key: &mut input.state.continuation.response_affinity_key,
            requires_affinity_owner: &mut input.state.continuation.requires_affinity_owner,
        },
        eligible,
        input.runtime,
        &input.key.id,
        input.request_json,
        input.resolved_model,
    )
}

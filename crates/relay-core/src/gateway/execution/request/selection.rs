use super::prelude::*;
use super::recovery::{
    adapter_error_response, replay_native_affinity_continuation,
    should_wait_for_candidate_availability,
};

pub(super) enum SelectionMiss {
    Continue,
    Stop { retry_window_expired: bool },
    Respond(Response<Body>),
}

pub(super) struct SelectionMissInput<'a> {
    pub(super) budget: &'a SharedRequestBudget,
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) resolved_model: &'a str,
    pub(super) client_wire_api: WireApi,
    pub(super) stream: bool,
    pub(super) request: &'a mut Value,
    pub(super) response_affinity_key: &'a mut Option<String>,
    pub(super) requires_affinity_owner: &'a mut bool,
    pub(super) allow_previous_response_reset: bool,
    pub(super) has_previous_response_id: bool,
    pub(super) has_unpaired_tool_output: &'a mut bool,
    pub(super) repairs: &'a mut AttemptRepairs,
    pub(super) tried: &'a mut HashSet<String>,
    pub(super) incompatible: &'a HashSet<String>,
    pub(super) retry_context: &'a CandidateRetryContext<'a>,
    pub(super) last_failure: &'a Option<AttemptFailure>,
    pub(super) last_adapter_error: &'a mut Option<AdapterError>,
    pub(super) retry_until_available: bool,
    pub(super) attempt: u16,
    pub(super) retry_window_expired: bool,
}

/// No reserved candidate is available for this attempt. Recover a pinned
/// continuation, wait for a route, or stop with the same response the loop
/// used to return directly.
pub(super) async fn handle_selection_miss(input: SelectionMissInput<'_>) -> SelectionMiss {
    let mut input = input;
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
    !*input.requires_affinity_owner
        && input.runtime.release_unroutable_response_affinity(
            input.key,
            input.response_affinity_key,
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
        || !*input.requires_affinity_owner
        || input.repairs.native_replay
    {
        return None;
    }
    match replay_native_affinity_continuation(
        input.runtime,
        &input.key.id,
        input.request,
        input.response_affinity_key.as_deref(),
        input.resolved_model,
        input.stream,
        &mut input.repairs.native_replay,
    ) {
        Ok(true) => {
            clear_materialized_continuation(
                input.response_affinity_key,
                input.requires_affinity_owner,
                input.has_unpaired_tool_output,
            );
            Some(SelectionMiss::Continue)
        }
        Ok(false) => None,
        Err(error) => Some(SelectionMiss::Respond(adapter_error_response(error))),
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
        .response_affinity_key
        .as_deref()
        .and_then(|affinity_key| owner_supports_route(input, affinity_key));
    reset_pinned_continuation(input, owner_supports_route == Some(false))
}

fn reject_unavailable_pinned_owner(input: &SelectionMissInput<'_>) -> Option<SelectionMiss> {
    let unavailable = *input.requires_affinity_owner
        && input
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
        &CandidateRetryContext {
            runtime: input.retry_context.runtime,
            key: input.retry_context.key,
            resolved_model: input.retry_context.resolved_model,
            protocols: input.retry_context.protocols,
            operation: input.retry_context.operation,
            exclusions: input.incompatible,
        },
        input.tried,
        input.response_affinity_key.as_deref(),
    )
    .await
}

async fn wait_for_candidate_availability(
    input: &mut SelectionMissInput<'_>,
) -> Option<SelectionMiss> {
    if !should_wait_for_candidate_availability(
        input.retry_until_available,
        input.last_failure,
        input.last_adapter_error.is_some(),
        input.has_previous_response_id,
    ) {
        return None;
    }
    if !wait_for_candidate_retry(
        input.budget,
        input.retry_context,
        input.tried,
        input.response_affinity_key.as_deref(),
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
    if input.attempt != 0 {
        return None;
    }
    let (retry_at, reason) = input.runtime.all_applicable_cooldown(
        input.key,
        input.resolved_model,
        candidate_protocols(input.client_wire_api),
        input.tried,
        input.response_affinity_key.as_deref(),
        now_ms(),
        crate::scheduler::rotation::RotationOperation::Text,
    )?;
    Some(SelectionMiss::Respond(cooldown_error(
        retry_at,
        None,
        reason == crate::scheduler::CooldownReason::RateLimit,
        crate::ErrorOrigin::Relay,
    )))
}

fn stop_without_candidate(input: &mut SelectionMissInput<'_>) -> SelectionMiss {
    if input.last_failure.is_none() {
        if let Some(error) = input.last_adapter_error.take() {
            return SelectionMiss::Respond(adapter_error_response(error));
        }
    }
    SelectionMiss::Stop {
        retry_window_expired: input.retry_window_expired,
    }
}

fn pinned_responses_reset_allowed(input: &SelectionMissInput<'_>) -> bool {
    input.client_wire_api == WireApi::Responses
        && input.allow_previous_response_reset
        && input.has_previous_response_id
        && *input.requires_affinity_owner
        && !*input.has_unpaired_tool_output
        && !input.repairs.model_switch_reset
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
            attempted: &mut input.repairs.model_switch_reset,
            response_affinity_key: input.response_affinity_key,
            requires_affinity_owner: input.requires_affinity_owner,
        },
        eligible,
        input.runtime,
        &input.key.id,
        input.request,
        input.resolved_model,
    )
}

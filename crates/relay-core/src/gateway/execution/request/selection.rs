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
    pub(super) request_json: &'a mut Value,
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
pub(super) async fn handle_selection_miss(
    selection_context: SelectionMissInput<'_>,
) -> SelectionMiss {
    let mut selection_context = selection_context;
    if let Some(error) = crate::gateway::errors::admission_error(selection_context.budget) {
        return SelectionMiss::Respond(error);
    }
    if release_unroutable_affinity(&mut selection_context) {
        return SelectionMiss::Continue;
    }
    if reset_owner_without_model(&mut selection_context) {
        return SelectionMiss::Continue;
    }
    if let Some(step) = replay_pinned_continuation(&mut selection_context) {
        return step;
    }
    if reset_owner_outside_scope(&mut selection_context) {
        return SelectionMiss::Continue;
    }
    if let Some(step) = reject_unavailable_pinned_owner(&selection_context) {
        return step;
    }
    if wait_for_route_recovery(&mut selection_context).await {
        return SelectionMiss::Continue;
    }
    if let Some(step) = wait_for_candidate_availability(&mut selection_context).await {
        return step;
    }
    if let Some(step) = first_attempt_cooldown(&selection_context) {
        return step;
    }
    stop_without_candidate(&mut selection_context)
}

fn release_unroutable_affinity(selection_context: &mut SelectionMissInput<'_>) -> bool {
    !*selection_context.requires_affinity_owner
        && selection_context
            .runtime
            .release_unroutable_response_affinity(
                selection_context.key,
                selection_context.response_affinity_key,
                selection_context.resolved_model,
                candidate_protocols(selection_context.client_wire_api),
                now_ms(),
            )
}

/// Native replay deliberately refuses to materialize a response across
/// model/protocol routes. When the bound owner itself no longer supports this
/// route, retaining its opaque response id would therefore block all eligible
/// new-model candidates before an upstream request is even attempted. Start a
/// fresh safe turn instead, but never infer that from temporary availability.
fn reset_owner_without_model(selection_context: &mut SelectionMissInput<'_>) -> bool {
    if !pinned_responses_reset_allowed(selection_context) {
        return false;
    }
    let owner_supports_model =
        selection_context
            .response_affinity_key
            .as_deref()
            .and_then(|affinity_key| {
                selection_context
                    .runtime
                    .response_affinity_owner_supports_model(
                        affinity_key,
                        selection_context.resolved_model,
                        candidate_protocols(selection_context.client_wire_api),
                        now_ms(),
                    )
            });
    reset_pinned_continuation(selection_context, owner_supports_model == Some(false))
}

/// Pool membership can change between two Codex turns. An opaque
/// previous_response_id remains pinned to its old owner, so normal selection
/// correctly declines to send it to a new provider. If Relay still has the
/// bounded, owner-scoped native replay for that turn, materialize it before
/// trying the replacement pool. This keeps the continuation safe while avoiding
/// a permanent no-candidate failure after an operator rotates API sources.
fn replay_pinned_continuation(
    selection_context: &mut SelectionMissInput<'_>,
) -> Option<SelectionMiss> {
    if selection_context.client_wire_api != WireApi::Responses
        || !selection_context.has_previous_response_id
        || !*selection_context.requires_affinity_owner
        || selection_context.repairs.native_replay
    {
        return None;
    }
    match replay_native_affinity_continuation(
        selection_context.runtime,
        &selection_context.key.id,
        selection_context.request_json,
        selection_context.response_affinity_key.as_deref(),
        selection_context.resolved_model,
        selection_context.stream,
        &mut selection_context.repairs.native_replay,
    ) {
        Ok(true) => {
            clear_materialized_continuation(
                selection_context.response_affinity_key,
                selection_context.requires_affinity_owner,
                selection_context.has_unpaired_tool_output,
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
fn reset_owner_outside_scope(selection_context: &mut SelectionMissInput<'_>) -> bool {
    if !pinned_responses_reset_allowed(selection_context) {
        return false;
    }
    let owner_supports_route = selection_context
        .response_affinity_key
        .as_deref()
        .and_then(|affinity_key| owner_supports_route(selection_context, affinity_key));
    reset_pinned_continuation(selection_context, owner_supports_route == Some(false))
}

fn reject_unavailable_pinned_owner(
    selection_context: &SelectionMissInput<'_>,
) -> Option<SelectionMiss> {
    let unavailable = *selection_context.requires_affinity_owner
        && selection_context
            .response_affinity_key
            .as_deref()
            .and_then(|affinity_key| owner_supports_route(selection_context, affinity_key))
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

async fn wait_for_route_recovery(selection_context: &mut SelectionMissInput<'_>) -> bool {
    wait_for_recovery(
        selection_context.budget,
        &CandidateRetryContext {
            runtime: selection_context.retry_context.runtime,
            key: selection_context.retry_context.key,
            resolved_model: selection_context.retry_context.resolved_model,
            protocols: selection_context.retry_context.protocols,
            operation: selection_context.retry_context.operation,
            exclusions: selection_context.incompatible,
        },
        selection_context.tried,
        selection_context.response_affinity_key.as_deref(),
    )
    .await
}

async fn wait_for_candidate_availability(
    selection_context: &mut SelectionMissInput<'_>,
) -> Option<SelectionMiss> {
    if !should_wait_for_candidate_availability(
        selection_context.retry_until_available,
        selection_context.last_failure,
        selection_context.last_adapter_error.is_some(),
        selection_context.has_previous_response_id,
    ) {
        return None;
    }
    if !wait_for_candidate_retry(
        selection_context.budget,
        selection_context.retry_context,
        selection_context.tried,
        selection_context.response_affinity_key.as_deref(),
    )
    .await
    {
        return Some(SelectionMiss::Stop {
            retry_window_expired: true,
        });
    }
    Some(SelectionMiss::Continue)
}

fn first_attempt_cooldown(selection_context: &SelectionMissInput<'_>) -> Option<SelectionMiss> {
    if selection_context.attempt != 0 {
        return None;
    }
    let (retry_at, reason) = selection_context.runtime.all_applicable_cooldown(
        selection_context.key,
        selection_context.resolved_model,
        candidate_protocols(selection_context.client_wire_api),
        selection_context.tried,
        selection_context.response_affinity_key.as_deref(),
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

fn stop_without_candidate(selection_context: &mut SelectionMissInput<'_>) -> SelectionMiss {
    if selection_context.last_failure.is_none() {
        if let Some(error) = selection_context.last_adapter_error.take() {
            return SelectionMiss::Respond(adapter_error_response(error));
        }
    }
    SelectionMiss::Stop {
        retry_window_expired: selection_context.retry_window_expired,
    }
}

fn pinned_responses_reset_allowed(selection_context: &SelectionMissInput<'_>) -> bool {
    selection_context.client_wire_api == WireApi::Responses
        && selection_context.allow_previous_response_reset
        && selection_context.has_previous_response_id
        && *selection_context.requires_affinity_owner
        && !*selection_context.has_unpaired_tool_output
        && !selection_context.repairs.model_switch_reset
}

fn owner_supports_route(
    selection_context: &SelectionMissInput<'_>,
    affinity_key: &str,
) -> Option<bool> {
    selection_context
        .runtime
        .response_affinity_owner_supports_route(
            selection_context.key,
            affinity_key,
            selection_context.resolved_model,
            candidate_protocols(selection_context.client_wire_api),
            now_ms(),
        )
}

fn reset_pinned_continuation(
    selection_context: &mut SelectionMissInput<'_>,
    eligible: bool,
) -> bool {
    reset_materialized_continuation(
        &mut ContinuationReset {
            attempted: &mut selection_context.repairs.model_switch_reset,
            response_affinity_key: selection_context.response_affinity_key,
            requires_affinity_owner: selection_context.requires_affinity_owner,
        },
        eligible,
        selection_context.runtime,
        &selection_context.key.id,
        selection_context.request_json,
        selection_context.resolved_model,
    )
}

use std::collections::HashSet;

use super::super::super::execution::AttemptRepairs;
use super::super::*;

pub(super) enum GapAction {
    Continue,
    Break,
    Fail(GatewayFailure),
}

pub(super) struct SelectionGap<'a> {
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) request: &'a mut ClientRequest,
    pub(super) tried: &'a mut HashSet<String>,
    pub(super) last_failure: &'a Option<GatewayFailure>,
    pub(super) http_fallback_origin: &'a Option<ErrorOrigin>,
    pub(super) allow_previous_response_reset: bool,
    pub(super) repairs: &'a mut AttemptRepairs,
    pub(super) retry_window_expired: &'a mut bool,
    pub(super) wait_for_candidate_availability: bool,
}

pub(super) async fn recover_without_candidate(gap: &mut SelectionGap<'_>) -> GapAction {
    let runtime = gap.runtime;
    let key = gap.key;
    let last_failure = gap.last_failure;
    let websocket_http_fallback_origin = gap.http_fallback_origin;
    let allow_previous_response_reset = gap.allow_previous_response_reset;
    let wait_for_candidate_availability = gap.wait_for_candidate_availability;
    let request = &mut *gap.request;
    let tried = &mut *gap.tried;
    let repairs = &mut *gap.repairs;
    let retry_window_expired = &mut *gap.retry_window_expired;
    let model_switch_reset_attempted = &mut repairs.model_switch_reset;
    let native_replay_attempted = &mut repairs.native_replay;
    if let Some(reason) = request.budget.admission_stop_reason() {
        return GapAction::Fail(GatewayFailure::admission(reason));
    }
    if !request.requires_affinity_owner
        && runtime.release_unroutable_response_affinity(
            key,
            &mut request.response_affinity_key,
            &request.resolved_model,
            WEBSOCKET_PROTOCOLS,
            now_ms(),
        )
    {
        return GapAction::Continue;
    }
    // A previous response is pinned to its original owner.  When a
    // later WebSocket turn switches to a model that owner cannot
    // structurally serve, affinity selection returns no candidate
    // before an upstream request is attempted.  Clear the opaque
    // continuation once so the new model can use a compatible owner;
    // temporary health, quota, and cooldown misses remain retryable.
    if allow_previous_response_reset
        && request.has_previous_response_id()
        && !request.has_unpaired_tool_output()
        && !*model_switch_reset_attempted
        && (request
            .response_affinity_key
            .as_deref()
            .and_then(|affinity_key| {
                runtime.response_affinity_owner_supports_model(
                    affinity_key,
                    &request.resolved_model,
                    WEBSOCKET_PROTOCOLS,
                    now_ms(),
                )
            })
            == Some(false)
            || request
                .response_affinity_key
                .as_deref()
                .and_then(|affinity_key| {
                    runtime.response_affinity_owner_supports_route(
                        key,
                        affinity_key,
                        &request.resolved_model,
                        WEBSOCKET_PROTOCOLS,
                        now_ms(),
                    )
                })
                == Some(false))
        && request.drop_previous_response_id(runtime, &key.id)
    {
        *model_switch_reset_attempted = true;
        return GapAction::Continue;
    }
    // The owner may be temporarily ineligible because its quota or
    // cooldown changed after the previous turn. Use the bounded native
    // replay before waiting, then let the next selection choose any
    // compatible candidate (OAuth or API source).
    if request.has_previous_response_id() && request.requires_affinity_owner {
        if let Some(affinity_key) = request.response_affinity_key.clone() {
            if let Some(owner_candidate_id) =
                runtime.response_affinity_candidate(&affinity_key, now_ms())
            {
                let owner_model = runtime
                    .executor_route(
                        &owner_candidate_id,
                        &request.resolved_model,
                        &key.scope_snapshot(),
                        WEBSOCKET_PROTOCOLS,
                        false,
                    )
                    .map(|route| route.source_model)
                    .unwrap_or_else(|| request.resolved_model.clone());
                match request.replay_native_continuation(
                    runtime,
                    &key.id,
                    &owner_candidate_id,
                    &owner_model,
                ) {
                    Ok(true) => {
                        *native_replay_attempted = true;
                        tried.clear();
                        return GapAction::Continue;
                    }
                    Ok(false) => {}
                    Err(failure) => return GapAction::Fail(failure),
                }
            }
        }
    }
    if request.requires_affinity_owner
        && request
            .response_affinity_key
            .as_deref()
            .and_then(|affinity_key| {
                runtime.response_affinity_owner_supports_route(
                    key,
                    affinity_key,
                    &request.resolved_model,
                    WEBSOCKET_PROTOCOLS,
                    now_ms(),
                )
            })
            == Some(false)
    {
        return GapAction::Fail(GatewayFailure::continuation_unavailable());
    }
    let may_wait_for_route = last_failure.as_ref().is_none_or(|failure| {
        super::super::super::errors::retryable_recovery_wait(
            failure.status,
            failure.category,
            request.has_previous_response_id(),
        )
    });
    if websocket_http_fallback_origin.is_none()
        && may_wait_for_route
        && wait_for_recovery(
            &request.budget,
            &CandidateRetryContext {
                runtime,
                key,
                resolved_model: &request.resolved_model,
                protocols: WEBSOCKET_PROTOCOLS,
                operation: crate::scheduler::rotation::RotationOperation::Text,
                exclusions: &HashSet::new(),
            },
            &mut *tried,
            request.response_affinity_key.as_deref(),
        )
        .await
    {
        return GapAction::Continue;
    }
    if websocket_http_fallback_origin.is_none()
        && may_wait_for_route
        && wait_for_candidate_availability
    {
        tried.clear();
        if !runtime
            .wait_for_recovery_event(
                key,
                &request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                tried,
                request.response_affinity_key.as_deref(),
                crate::scheduler::rotation::RotationOperation::Text,
                &request.budget,
                None,
                true,
            )
            .await
        {
            *retry_window_expired = true;
            return GapAction::Break;
        }
        request.budget.begin_recovery_pass();
        return GapAction::Continue;
    }
    GapAction::Break
}

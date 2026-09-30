use super::super::super::errors::AttemptFailure;
use super::super::super::now_ms;
use super::super::request::should_wait_for_candidate_availability;
use super::super::{
    reset_materialized_continuation, wait_for_candidate_retry, wait_for_recovery,
    CandidateRetryContext, ContinuationReset,
};
use crate::runtime::AuthenticatedKey;
use crate::scheduler::rotation::SharedRequestBudget;
use crate::GatewayRuntime;
use crate::WireApi;
use axum::body::Body;
use axum::http::Response;
use serde_json::Value;
use std::collections::HashSet;

pub(super) enum AccountSelectionMiss {
    Continue,
    Stop { retry_window_expired: bool },
    Respond(Response<Body>),
}

use super::super::AttemptRepairs;

pub(super) struct AccountSelectionMissInput<'a> {
    pub(super) budget: &'a SharedRequestBudget,
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) request: &'a mut Value,
    pub(super) resolved_model: &'a str,
    pub(super) response_affinity_key: &'a mut Option<String>,
    pub(super) requires_affinity_owner: &'a mut bool,
    pub(super) has_previous_response_id: bool,
    pub(super) has_unpaired_tool_output: bool,
    pub(super) repairs: &'a mut AttemptRepairs,
    pub(super) tried: &'a mut HashSet<String>,
    pub(super) retry_context: &'a CandidateRetryContext<'a>,
    pub(super) last_failure: &'a Option<AttemptFailure>,
    pub(super) wait_for_candidate_availability: bool,
}

/// No account candidate is reserved for this attempt. Recover a pinned
/// continuation, wait for the owner, or stop with the same response the loop
/// used to return directly.
pub(super) async fn handle_account_selection_miss(
    input: AccountSelectionMissInput<'_>,
) -> AccountSelectionMiss {
    let AccountSelectionMissInput {
        budget,
        runtime,
        key,
        request,
        resolved_model,
        response_affinity_key,
        requires_affinity_owner,
        has_previous_response_id,
        has_unpaired_tool_output,
        repairs,
        tried,
        retry_context,
        last_failure,
        wait_for_candidate_availability,
    } = input;
    let model_switch_reset_attempted = &mut repairs.model_switch_reset;
    if let Some(error) = crate::gateway::errors::admission_error(budget) {
        return AccountSelectionMiss::Respond(error);
    }
    if !*requires_affinity_owner
        && runtime.release_unroutable_response_affinity(
            key,
            response_affinity_key,
            resolved_model,
            &[WireApi::Responses],
            now_ms(),
        )
    {
        return AccountSelectionMiss::Continue;
    }
    // Account-only continuations are pinned to their creating account while it
    // remains in the key scope. If pool membership removes that owner, drop
    // the opaque response id once so another account can continue the chat
    // instead of waiting forever on an impossible affinity selection.
    // Temporary health, quota, and cooldown misses remain retryable on the
    // original owner.
    if has_previous_response_id && !has_unpaired_tool_output && !*model_switch_reset_attempted {
        let owner_supports_route = response_affinity_key.as_deref().and_then(|affinity_key| {
            runtime.response_affinity_owner_supports_route(
                key,
                affinity_key,
                resolved_model,
                &[WireApi::Responses],
                now_ms(),
            )
        });
        if reset_materialized_continuation(
            &mut ContinuationReset {
                attempted: model_switch_reset_attempted,
                response_affinity_key,
                requires_affinity_owner,
            },
            owner_supports_route == Some(false),
            runtime,
            &key.id,
            request,
            resolved_model,
        ) {
            return AccountSelectionMiss::Continue;
        }
    }
    if wait_for_recovery(
        budget,
        retry_context,
        tried,
        response_affinity_key.as_deref(),
    )
    .await
    {
        return AccountSelectionMiss::Continue;
    }
    if should_wait_for_candidate_availability(
        wait_for_candidate_availability,
        last_failure,
        false,
        has_previous_response_id,
    ) {
        if !wait_for_candidate_retry(
            budget,
            retry_context,
            tried,
            response_affinity_key.as_deref(),
        )
        .await
        {
            return AccountSelectionMiss::Stop {
                retry_window_expired: true,
            };
        }
        return AccountSelectionMiss::Continue;
    }
    AccountSelectionMiss::Stop {
        retry_window_expired: false,
    }
}

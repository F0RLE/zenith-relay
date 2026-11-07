//! Account-only request execution for Responses, compact, search, and wake.
//! Account-specific encrypted-history recovery lives in `encrypted_context`;
//! the Basis Points transport adapter remains isolated in `basis_points`.

mod dispatch;
mod encrypted_context;
mod failure;
mod prepare;
mod selection;
mod success;

use super::super::continuation::{
    RESPONSE_CONTINUATION_UNAVAILABLE_CODE, RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
    prepare_response_continuation, prepare_response_owner_change, previous_response_id,
};
use super::super::errors::{AttemptFailure, api_error};
use super::super::now_ms;
use super::super::request::{
    AccountEndpoint, RequestToolPolicy, ServiceTierPolicy, client_request_identity, request_id,
};
use super::CandidateRetryContext;
use super::compatibility::{
    RejectedRequest, record_adapter_rejection, record_continuation_rejection,
};
use super::request::adapter_error_response;
use super::state::AttemptState;
use super::{RequestFailureInput, finish_request_failure};
use crate::runtime::AuthenticatedKey;
use crate::scheduler::rotation::{RotationOperation, SharedRequestBudget};
use crate::{GatewayRuntime, WireApi};
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use dispatch::{
    AccountDispatch, AccountDispatchInput, DispatchedAccountAttempt, dispatch_account_attempt,
};
use failure::{AccountStatusFailure, AccountStatusFailureInput, handle_account_status_failure};
use prepare::{
    AccountPrepare, AccountPrepareInput, PreparedAccountAttempt, prepare_account_attempt,
};
use selection::{AccountSelectionMiss, AccountSelectionMissInput, handle_account_selection_miss};
use serde_json::Value;
use std::sync::Arc;
use success::{AccountSuccessInput, complete_account_response};

pub(in crate::gateway::execution) use encrypted_context::drop_rejected_encrypted_context;

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
    /// Optional internal origin attached to the request's usage record.  This
    /// keeps scheduler-owned work distinguishable from customer traffic while
    /// preserving the same account execution and retry pipeline.
    pub(in crate::gateway) request_origin: Option<&'static str>,
}

pub(in crate::gateway) async fn execute_account_endpoint(
    context: AccountExecution,
) -> Response<Body> {
    let started = std::time::Instant::now();
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
        request_origin,
    } = context;
    if let Err(error) = runtime
        .agent_message_context(&key.id)
        .restore_request(&mut request)
    {
        return adapter_error_response(error, None);
    }
    let mut service_tier_policy = ServiceTierPolicy::pool_owned(&request);
    service_tier_policy.capture_pool_default(
        &runtime,
        &resolved_model,
        &runtime.account_service_tiers(&key, &resolved_model),
    );
    let request_id = request_id();
    if let Some(origin) = request_origin {
        runtime.mark_request_origin(&request_id, origin);
    }
    let mut tool_policy = RequestToolPolicy::new(&runtime, &request);
    let client_identity = client_request_identity(&client_headers, &request);
    let client_context_id = client_identity.context_id;
    let prompt_affinity_key = runtime.prompt_affinity_key(
        &key.id,
        &resolved_model,
        request.get("prompt_cache_key").and_then(Value::as_str),
        client_identity.affinity_id.as_deref(),
    );
    let continuation =
        match prepare_response_continuation(&runtime, &key.id, &mut request, now_ms(), None) {
            Ok(continuation) => continuation,
            Err(()) => {
                record_continuation_rejection(
                    &runtime,
                    RejectedRequest {
                        request_id: &request_id,
                        attempt: 0,
                        local_key_id: &key.id,
                        requested_model: &requested_model,
                        resolved_model: Some(&resolved_model),
                        client_context_id: client_context_id.as_deref(),
                        wire_api: WireApi::Responses,
                        transport: crate::UsageTransport::Http,
                        service_tier: service_tier_policy.selected_tier(),
                    },
                    started.elapsed().as_millis() as u64,
                );
                return api_error(
                    StatusCode::CONFLICT,
                    RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
                    RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
                );
            }
        };
    let mut state = AttemptState::from_prepared(continuation);
    let account_only_exclusions = runtime.api_source_candidate_ids();
    state.retry.tried = account_only_exclusions.clone();
    let budget = SharedRequestBudget::for_incoming_request(runtime.request_dispatch_budget());
    let operation = if endpoint == AccountEndpoint::Compact {
        RotationOperation::Compaction
    } else {
        RotationOperation::Text
    };
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
        // Wake checks one named account. Compact and search can leave that
        // account once, before selection, when ordinary admission prefers
        // another physical member and the saved history can be resent.
        if !state.continuation.repairs.quota_yield {
            state.continuation.repairs.quota_yield = true;
            if endpoint != AccountEndpoint::Wake
                && let Some(affinity_key) = state.continuation.response_affinity_key.clone()
                && runtime.automatic_response_owner_should_yield(
                    &key,
                    &affinity_key,
                    &resolved_model,
                    &[WireApi::Responses],
                    &state.retry.tried,
                    now_ms(),
                )
                && prepare_response_owner_change(
                    &runtime,
                    &key.id,
                    &mut request,
                    &resolved_model,
                    state.continuation.requires_affinity_owner,
                    now_ms(),
                )
            {
                state.continuation.clear_materialized();
            }
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
        let mut retry_exclusions = account_only_exclusions.clone();
        retry_exclusions.extend(state.retry.preparation_exclusions.iter().cloned());
        let mut selection_exclusions = state.retry.tried.clone();
        selection_exclusions.extend(retry_exclusions.iter().cloned());
        let Some((selected_candidate, lease)) = runtime
            .select_and_reserve_operation_with_budget(
                &key,
                &resolved_model,
                &[WireApi::Responses],
                &selection_exclusions,
                (
                    state.continuation.response_affinity_key.as_deref(),
                    prompt_affinity_key.as_deref(),
                ),
                now_ms(),
                operation,
                &budget,
            )
            .await
        else {
            state.continuation.release_repair_owner(&runtime);
            let has_preparation_error = state.retry.last_adapter_error.is_some()
                || !state.retry.preparation_exclusions.is_empty();
            match handle_account_selection_miss(AccountSelectionMissInput {
                budget: &budget,
                runtime: &runtime,
                key: &key,
                request: &mut request,
                resolved_model: &resolved_model,
                state: &mut state,
                has_previous_response_id,
                retry_context: &CandidateRetryContext {
                    runtime: &runtime,
                    key: &key,
                    resolved_model: &resolved_model,
                    protocols: &[WireApi::Responses],
                    operation,
                    exclusions: &retry_exclusions,
                },
                has_preparation_error,
                wait_for_candidate_availability,
            })
            .await
            {
                AccountSelectionMiss::Continue => continue,
                AccountSelectionMiss::Stop {
                    retry_window_expired: expired,
                } => {
                    state.retry.window_expired = expired;
                    break;
                }
                AccountSelectionMiss::Respond(response) => return response,
            }
        };
        state.continuation.detach_repair_owner();
        state
            .retry
            .tried
            .insert(selected_candidate.candidate_id.clone());
        let response_affinity_hit = selected_candidate.response_affinity_hit;
        let preparation_started = runtime.diagnostics_enabled().then(std::time::Instant::now);
        let prepared_attempt = match prepare_account_attempt(AccountPrepareInput {
            runtime: &runtime,
            key: &key,
            request: &mut request,
            resolved_model: &resolved_model,
            endpoint,
            responses_lite: &responses_lite,
            rewrite_model,
            service_tier_policy: &service_tier_policy,
            tool_policy: &mut tool_policy,
            candidate_id: &selected_candidate.candidate_id,
            half_open_probe: selected_candidate.half_open_probe,
            diagnostics: selected_candidate.diagnostics,
            client_context_id: &client_context_id,
            retry: &mut state.retry,
        }) {
            AccountPrepare::Continue => {
                state
                    .retry
                    .preparation_exclusions
                    .insert(selected_candidate.candidate_id.clone());
                state.continuation.release_repair_owner(&runtime);
                continue;
            }
            AccountPrepare::Reject(error) => {
                state.retry.last_adapter_error = Some(error);
                break;
            }
            AccountPrepare::Respond(response) => {
                state.continuation.release_repair_owner(&runtime);
                return response;
            }
            AccountPrepare::Ready(prepared) => *prepared,
        };
        let PreparedAccountAttempt {
            preparation,
            bps_catalog,
            route,
            basis_points_route,
            route_responses_lite,
            selected_error_origin,
            upstream_url,
            upstream_body,
            request_body,
            reasoning_effort,
            tool_use,
        } = prepared_attempt;

        let dispatches_before = budget.dispatches();
        let dispatch = dispatch_account_attempt(AccountDispatchInput {
            preparation_started,
            preparation,
            bps_catalog,
            runtime: &runtime,
            key: &key,
            lease: &lease,
            budget: &budget,
            route,
            endpoint,
            basis_points_route,
            route_responses_lite,
            selected_error_origin,
            upstream_url,
            upstream_body,
            request_body,
            reasoning_effort,
            tool_use,
            request_id: &request_id,
            original_request: &request,
            requested_model: &requested_model,
            resolved_model: &resolved_model,
            client_headers: &client_headers,
            operation,
            account_only_exclusions: &account_only_exclusions,
            continuation: &state.continuation,
            retry: &mut state.retry,
        })
        .await;
        state.retry.attempt = u16::from(budget.dispatches());
        let dispatched_attempt = match dispatch {
            AccountDispatch::Continue => {
                // A failed preflight cannot spend the generation budget or
                // vote on model health, but it must not spin recovery either.
                if budget.dispatches() == dispatches_before {
                    state
                        .retry
                        .preparation_exclusions
                        .insert(selected_candidate.candidate_id.clone());
                }
                state.continuation.release_repair_owner(&runtime);
                continue;
            }
            AccountDispatch::Respond(response) => {
                state.continuation.release_repair_owner(&runtime);
                return response;
            }
            AccountDispatch::Ready(dispatched) => *dispatched,
        };
        let DispatchedAccountAttempt {
            bps_catalog,
            route,
            status,
            response_headers,
            bytes,
            attempt,
            started,
            reasoning_effort,
            tool_use,
            selected_error_origin,
            basis_points_route,
        } = dispatched_attempt;
        if !status.is_success() {
            match handle_account_status_failure(AccountStatusFailureInput {
                status,
                bytes,
                endpoint,
                response_headers: &response_headers,
                runtime: &runtime,
                lease: &lease,
                route: &route,
                attempt,
                request_id: &request_id,
                key: &key,
                reasoning_effort: &reasoning_effort,
                requested_model: &requested_model,
                tool_use: &tool_use,
                started,
                request: &mut request,
                resolved_model: &resolved_model,
                client_headers: &client_headers,
                selected_error_origin,
                response_affinity_hit,
                has_previous_response_id,
                prompt_affinity_key: &prompt_affinity_key,
                state: &mut state,
            }) {
                AccountStatusFailure::Continue => continue,
                AccountStatusFailure::Respond(response) => {
                    state.continuation.release_repair_owner(&runtime);
                    return response;
                }
            }
        }
        let response = complete_account_response(AccountSuccessInput {
            bps_catalog,
            endpoint,
            status,
            bytes,
            response_headers: &response_headers,
            runtime: &runtime,
            lease: &lease,
            route: &route,
            attempt,
            request_id: &request_id,
            key: &key,
            reasoning_effort: &reasoning_effort,
            requested_model: &requested_model,
            tool_use,
            started,
            request: &request,
            client_headers: &client_headers,
            selected_error_origin,
            basis_points_route,
            prompt_affinity_key: &prompt_affinity_key,
        });
        state.continuation.release_repair_owner(&runtime);
        return response;
    }

    state.continuation.release_repair_owner(&runtime);

    if let Some(error) = crate::gateway::errors::admission_error(&budget) {
        return error;
    }
    if let Some(error) = state.retry.last_adapter_error {
        record_adapter_rejection(
            &runtime,
            RejectedRequest {
                request_id: &request_id,
                attempt: u16::from(budget.dispatches()),
                local_key_id: &key.id,
                requested_model: &requested_model,
                resolved_model: Some(&resolved_model),
                client_context_id: client_context_id.as_deref(),
                wire_api: WireApi::Responses,
                transport: crate::UsageTransport::Http,
                service_tier: service_tier_policy.selected_tier(),
            },
            &error,
            started.elapsed().as_millis() as u64,
        );
        return adapter_error_response(error, Some(&request_id));
    }
    let failure = AttemptFailure::after_exhausted_attempts(
        state.retry.window_expired,
        state.retry.last_failure,
    );
    finish_request_failure(RequestFailureInput {
        runtime: &runtime,
        key: &key,
        resolved_model: &resolved_model,
        protocols: &[WireApi::Responses],
        operation,
        exclusions: &account_only_exclusions,
        response_affinity_key: state.continuation.response_affinity_key.as_deref(),
        failure,
        preserved: state.retry.last_preserved_upstream_error.as_ref(),
        failure_origin: state.retry.last_failure_origin,
        request_id: &request_id,
    })
}

fn request_has_previous_response_id(request: &Value) -> bool {
    previous_response_id(request).is_some()
}

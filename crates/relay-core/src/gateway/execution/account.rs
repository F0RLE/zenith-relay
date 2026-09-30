mod dispatch;
mod failure;
mod prepare;
mod selection;
mod success;

use super::super::continuation::{
    prepare_response_continuation, previous_response_id, RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
    RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
};
use super::super::errors::{api_error, AttemptFailure, PreservedUpstreamError};
use super::super::now_ms;
use super::super::request::{
    client_context_fingerprint, request_id, AccountEndpoint, RequestToolPolicy, ServiceTierPolicy,
};
use super::request::adapter_error_response;
use super::AttemptRepairs;
use super::CandidateRetryContext;
use super::{finish_request_failure, RequestFailureInput};
use crate::runtime::AuthenticatedKey;
use crate::scheduler::rotation::{RotationOperation, SharedRequestBudget};
use crate::{GatewayRuntime, WireApi};
use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use dispatch::{
    dispatch_account_attempt, AccountDispatch, AccountDispatchInput, DispatchedAccountAttempt,
};
use failure::{handle_account_status_failure, AccountStatusFailure, AccountStatusFailureInput};
use prepare::{
    prepare_account_attempt, AccountPrepare, AccountPrepareInput, PreparedAccountAttempt,
};
use selection::{handle_account_selection_miss, AccountSelectionMiss, AccountSelectionMissInput};
use serde_json::Value;
use std::sync::Arc;
use success::{complete_account_response, AccountSuccess, AccountSuccessInput};

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
    /// Account-only callers such as the background wake path deliberately
    /// disable the automatic Responses Lite contract.  Lite is a whole
    /// request contract and must never be inferred for a synthetic internal
    /// request merely because the account's catalog happened to confirm it.
    pub(in crate::gateway) allow_automatic_responses_lite: bool,
    /// Optional internal origin attached to the request's usage record.  This
    /// keeps scheduler-owned work distinguishable from customer traffic while
    /// preserving the same account execution and retry pipeline.
    pub(in crate::gateway) request_origin: Option<&'static str>,
}

pub(in crate::gateway) async fn execute_account_endpoint(
    context: AccountExecution,
) -> Response<Body> {
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
        allow_automatic_responses_lite,
        request_origin,
    } = context;
    let service_tier_policy = ServiceTierPolicy::pool_owned(&request);
    let request_id = request_id();
    if let Some(origin) = request_origin {
        runtime.mark_request_origin(&request_id, origin);
    }
    let mut tool_policy = RequestToolPolicy::new(&runtime, &request);
    let client_context_id = client_context_fingerprint(&client_headers);
    let prompt_affinity_key = runtime.prompt_affinity_key(
        &key.id,
        &resolved_model,
        request.get("prompt_cache_key").and_then(Value::as_str),
        client_context_id.as_deref(),
    );
    let continuation =
        match prepare_response_continuation(&runtime, &key.id, &mut request, now_ms(), None) {
            Ok(continuation) => continuation,
            Err(()) => {
                return api_error(
                    StatusCode::CONFLICT,
                    RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
                    RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
                )
            }
        };
    let mut response_affinity_key = continuation.response_affinity_key;
    let mut requires_affinity_owner = continuation.requires_affinity_owner;
    let mut has_unpaired_tool_output = continuation.has_unpaired_tool_output;
    let account_only_exclusions = runtime.api_source_candidate_ids();
    let mut tried = account_only_exclusions.clone();
    let budget = SharedRequestBudget::for_incoming_request(runtime.request_dispatch_budget());
    let mut repairs = AttemptRepairs::default();
    let mut basis_points_relay_retry_attempted = false;
    let mut basis_points_relay_retry_parameter: Option<&'static str> = None;
    let mut last_failure: Option<AttemptFailure> = None;
    let mut last_adapter_error = None;
    let mut last_preserved_upstream_error: Option<PreservedUpstreamError> = None;
    let mut last_failure_origin = crate::ErrorOrigin::Relay;
    let mut retry_window_expired = false;
    let operation = if endpoint == AccountEndpoint::Compact {
        RotationOperation::Compaction
    } else {
        RotationOperation::Text
    };
    let retry_context = CandidateRetryContext {
        runtime: &runtime,
        key: &key,
        resolved_model: &resolved_model,
        protocols: &[WireApi::Responses],
        operation,
        exclusions: &account_only_exclusions,
    };
    // Compact and search endpoints only select OAuth accounts, but their
    // retries can still move between account slots. Keep automatic Lite off
    // unless every such configured slot confirmed the same contract.
    let automatic_responses_lite = allow_automatic_responses_lite
        && runtime.codex_model_account_responses_routes_all_support_lite(&key, &resolved_model);

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
        // A model-switch or stale-tool recovery can remove the opaque
        // continuation id. Retry policy must then use the repaired request,
        // not the continuation state captured before the loop.
        let has_previous_response_id = request_has_previous_response_id(&request);
        // Account-only client endpoints share the API's live route-recovery
        // policy; internal wake requests remain ineligible.
        let retry_until_available = runtime.route_recovery_enabled();
        let wait_for_candidate_availability =
            wait_for_candidate_availability && retry_until_available;
        let Some((selected, lease)) = runtime
            .select_and_reserve_operation_with_budget(
                &key,
                &resolved_model,
                &[WireApi::Responses],
                &tried,
                (
                    response_affinity_key.as_deref(),
                    prompt_affinity_key.as_deref(),
                ),
                now_ms(),
                operation,
                &budget,
            )
            .await
        else {
            match handle_account_selection_miss(AccountSelectionMissInput {
                budget: &budget,
                runtime: &runtime,
                key: &key,
                request: &mut request,
                resolved_model: &resolved_model,
                response_affinity_key: &mut response_affinity_key,
                requires_affinity_owner: &mut requires_affinity_owner,
                has_previous_response_id,
                has_unpaired_tool_output,
                repairs: &mut repairs,
                tried: &mut tried,
                retry_context: &retry_context,
                last_failure: &last_failure,
                wait_for_candidate_availability,
            })
            .await
            {
                AccountSelectionMiss::Continue => continue,
                AccountSelectionMiss::Stop {
                    retry_window_expired: expired,
                } => {
                    retry_window_expired = expired;
                    break;
                }
                AccountSelectionMiss::Respond(response) => return response,
            }
        };
        tried.insert(selected.candidate_id.clone());
        let response_affinity_hit = selected.response_affinity_hit;
        let prepared = match prepare_account_attempt(AccountPrepareInput {
            runtime: &runtime,
            key: &key,
            request: &mut request,
            resolved_model: &resolved_model,
            endpoint,
            responses_lite: &responses_lite,
            rewrite_model,
            automatic_responses_lite,
            service_tier_policy: &service_tier_policy,
            tool_policy: &mut tool_policy,
            candidate_id: &selected.candidate_id,
            half_open_probe: selected.half_open_probe,
            diagnostics: selected.diagnostics,
            client_context_id: &client_context_id,
            basis_points_relay_retry_parameter,
            last_failure: &mut last_failure,
            last_adapter_error: &mut last_adapter_error,
        }) {
            AccountPrepare::Continue => continue,
            AccountPrepare::Respond(response) => return response,
            AccountPrepare::Ready(prepared) => *prepared,
        };
        let PreparedAccountAttempt {
            route,
            basis_points_route,
            route_responses_lite,
            selected_error_origin,
            upstream_url,
            upstream_body,
            request_body,
            reasoning_effort,
            tool_use,
        } = prepared;

        let dispatched = match dispatch_account_attempt(AccountDispatchInput {
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
            requested_model: &requested_model,
            resolved_model: &resolved_model,
            client_headers: &client_headers,
            operation,
            account_only_exclusions: &account_only_exclusions,
            response_affinity_key: response_affinity_key.as_deref(),
            last_failure: &mut last_failure,
            last_failure_origin: &mut last_failure_origin,
        })
        .await
        {
            AccountDispatch::Continue => continue,
            AccountDispatch::Respond(response) => return response,
            AccountDispatch::Ready(dispatched) => *dispatched,
        };
        let DispatchedAccountAttempt {
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
        } = dispatched;
        if !status.is_success() {
            match handle_account_status_failure(AccountStatusFailureInput {
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
                tool_use: &tool_use,
                started,
                endpoint: &endpoint,
                request: &mut request,
                resolved_model: &resolved_model,
                client_headers: &client_headers,
                selected_error_origin,
                response_affinity_hit,
                has_previous_response_id,
                prompt_affinity_key: &prompt_affinity_key,
                tried: &mut tried,
                has_unpaired_tool_output: &mut has_unpaired_tool_output,
                requires_affinity_owner: &mut requires_affinity_owner,
                tool_policy: &mut tool_policy,
                response_affinity_key: &mut response_affinity_key,
                repairs: &mut repairs,
                last_failure: &mut last_failure,
                last_failure_origin: &mut last_failure_origin,
                last_preserved_upstream_error: &mut last_preserved_upstream_error,
            }) {
                AccountStatusFailure::Continue => continue,
                AccountStatusFailure::Respond(response) => return response,
            }
        }
        match complete_account_response(AccountSuccessInput {
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
            tried: &mut tried,
            basis_points_relay_retry_attempted: &mut basis_points_relay_retry_attempted,
            basis_points_relay_retry_parameter: &mut basis_points_relay_retry_parameter,
            last_adapter_error: &mut last_adapter_error,
        }) {
            AccountSuccess::Continue => continue,
            AccountSuccess::Respond(response) => return response,
        }
    }

    if let Some(error) = crate::gateway::errors::admission_error(&budget) {
        return error;
    }
    if let Some(error) = last_adapter_error {
        return adapter_error_response(error);
    }
    let failure = AttemptFailure::after_exhausted_attempts(retry_window_expired, last_failure);
    finish_request_failure(RequestFailureInput {
        runtime: &runtime,
        key: &key,
        resolved_model: &resolved_model,
        protocols: &[WireApi::Responses],
        operation,
        exclusions: &account_only_exclusions,
        response_affinity_key: response_affinity_key.as_deref(),
        failure,
        preserved: last_preserved_upstream_error.as_ref(),
        failure_origin: last_failure_origin,
        request_id: &request_id,
    })
}

fn request_has_previous_response_id(request: &Value) -> bool {
    previous_response_id(request).is_some()
}

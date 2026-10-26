use super::super::compatibility::{
    incompatible_routes, record_adapter_rejection, AdapterRejection, RouteCompatibility,
};
use super::drive::{drive_selected_attempt, AttemptCarry, DriveAttemptInput, DrivenAttempt};
use super::prelude::*;
use super::recovery::{adapter_error_response, request_has_previous_response_id};
use super::selection::{handle_selection_miss, SelectionMiss, SelectionMissInput};
use super::RequestExecution;

pub(in crate::gateway::execution) async fn execute_request(
    context: RequestExecution,
) -> Response<Body> {
    let scope = context.runtime.track_request_usage(&context.request_id);
    let response = execute_request_inner(context).await;
    let streaming = response.status().is_success()
        && response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
    scope.finish(response.status().as_u16(), streaming);
    response
}

async fn execute_request_inner(context: RequestExecution) -> Response<Body> {
    let started = Instant::now();
    let RequestExecution {
        mut tool_policy,
        runtime,
        key,
        mut request,
        service_tier_policy,
        mut requested_model,
        resolved_model,
        stream,
        mut request_id,
        forwarded_headers,
        client_context_id,
        client_affinity_id,
        mut response_affinity_key,
        mut requires_affinity_owner,
        client_wire_api,
        responses_lite,
        allow_previous_response_reset,
        attempt_offset,
        budget,
        transport,
        transport_fallback,
        mut reserved_candidate,
    } = context;
    let mut tried: HashSet<String> = Default::default();
    // Local preparation failures belong to this request, not provider health.
    // Recovery may revisit dispatched members after their cooldown, but it
    // must never recycle a route that cannot prepare the unchanged request.
    let mut preparation_exclusions = HashSet::new();
    let mut attempt = attempt_offset;
    let mut confirmed_response_missing = false;
    let mut repairs = AttemptRepairs::default();
    let mut last_failure: Option<AttemptFailure> = None;
    let mut last_adapter_error: Option<AdapterError> = None;
    let mut last_preserved_upstream_error: Option<PreservedUpstreamError> = None;
    let mut last_failure_origin = crate::ErrorOrigin::Relay;
    let mut retry_window_expired = false;
    let mut has_unpaired_tool_output = !unpaired_tool_output_ids(&request).is_empty();
    let mut prompt_affinity_key = runtime.prompt_affinity_key(
        &key.id,
        &resolved_model,
        request.get("prompt_cache_key").and_then(Value::as_str),
        client_affinity_id.as_deref(),
    );
    loop {
        budget.retain_input_bytes(crate::gateway::request_body::retained_request_bytes(
            &request,
        ));
        budget.configure_retry_window(
            runtime.route_recovery_window_ms(),
            runtime.route_recovery_enabled(),
        );
        if !budget.can_dispatch() {
            break;
        }
        if !repairs.quota_yield {
            repairs.quota_yield = true;
            if let Some(affinity_key) = response_affinity_key.clone() {
                if runtime.automatic_response_owner_should_yield(
                    &key,
                    &affinity_key,
                    &resolved_model,
                    candidate_protocols(client_wire_api),
                    &tried,
                    now_ms(),
                ) && prepare_response_owner_change(
                    &runtime,
                    &key.id,
                    &mut request,
                    &resolved_model,
                    requires_affinity_owner,
                    now_ms(),
                ) {
                    clear_materialized_continuation(
                        &mut response_affinity_key,
                        &mut requires_affinity_owner,
                        &mut has_unpaired_tool_output,
                    );
                }
            }
        }
        // Recovery can deliberately remove an unusable opaque response id.
        // Derive continuation semantics from the request that will actually be
        // sent on this attempt, rather than from its original payload.
        let has_previous_response_id = request_has_previous_response_id(client_wire_api, &request);
        // The gateway setting is live for every text protocol. Turning it off
        // wakes an already-waiting request on its next availability event.
        let retry_until_available = runtime.route_recovery_enabled();
        // Every pre-output retry (including SSE and transport failures) must
        // release a replayable request's optional tool affinity. Otherwise
        // selection stops at its already-tried owner despite healthy routes.
        // Opaque response references and unpaired tool outputs stay pinned.
        if !requires_affinity_owner
            && last_failure
                .as_ref()
                .is_some_and(|failure| failure_category_requires_cooldown(failure.category))
        {
            response_affinity_key = None;
        }
        let (mut incompatible, mut admitted, admission_error) =
            incompatible_routes(RouteCompatibility {
                runtime: &runtime,
                key: &key,
                model: &resolved_model,
                client: client_wire_api,
                request: &request,
                stream,
                tier_policy: &service_tier_policy,
                now_ms: now_ms(),
            });
        incompatible.extend(preparation_exclusions.iter().cloned());
        let retry_context = CandidateRetryContext {
            runtime: &runtime,
            key: &key,
            resolved_model: &resolved_model,
            protocols: candidate_protocols(client_wire_api),
            operation: crate::scheduler::rotation::RotationOperation::Text,
            exclusions: &incompatible,
        };
        let mut selection_exclusions = tried.clone();
        selection_exclusions.extend(incompatible.iter().cloned());
        if admission_error.is_some() {
            last_adapter_error = admission_error;
        }
        // Revalidate HTTP compatibility and identity, but keep the existing
        // reservation. Releasing it and selecting again advances rotation twice
        // and can silently move an HTTP-only account to another pool member.
        // The normal dispatch fence still checks live scope and lease revisions.
        let reserved = reserved_candidate.take().filter(|reserved| {
            reserved.local_key_id == key.id
                && reserved.model == resolved_model
                && !selection_exclusions.contains(&reserved.selection.candidate_id)
                && admitted.contains_key(&reserved.selection.candidate_id)
        });
        let selected = if let Some(reserved) = reserved {
            Some((reserved.selection, reserved.lease))
        } else {
            runtime
                .select_and_reserve_with_budget(
                    &key,
                    &resolved_model,
                    candidate_protocols(client_wire_api),
                    &selection_exclusions,
                    (
                        response_affinity_key.as_deref(),
                        prompt_affinity_key.as_deref(),
                    ),
                    now_ms(),
                    &budget,
                )
                .await
        };
        let Some((selected, lease)) = selected else {
            release_encrypted_context_repair_owner(
                &mut repairs,
                &mut response_affinity_key,
                &mut requires_affinity_owner,
                &runtime,
            );
            match handle_selection_miss(SelectionMissInput {
                budget: &budget,
                runtime: &runtime,
                key: &key,
                resolved_model: &resolved_model,
                client_wire_api,
                stream,
                request_json: &mut request,
                response_affinity_key: &mut response_affinity_key,
                requires_affinity_owner: &mut requires_affinity_owner,
                allow_previous_response_reset,
                has_previous_response_id,
                has_unpaired_tool_output: &mut has_unpaired_tool_output,
                repairs: &mut repairs,
                tried: &mut tried,
                retry_context: &retry_context,
                last_failure: &last_failure,
                has_preparation_error: !preparation_exclusions.is_empty(),
                last_adapter_error: &mut last_adapter_error,
                retry_until_available,
                attempt,
                retry_window_expired,
            })
            .await
            {
                SelectionMiss::Continue => continue,
                SelectionMiss::Stop {
                    retry_window_expired: expired,
                } => {
                    retry_window_expired = expired;
                    break;
                }
                SelectionMiss::AdapterError(error) => {
                    record_adapter_rejection(
                        &runtime,
                        AdapterRejection {
                            request_id: &request_id,
                            attempt,
                            local_key_id: &key.id,
                            requested_model: &requested_model,
                            resolved_model: &resolved_model,
                            client_context_id: client_context_id.as_deref(),
                            wire_api: client_wire_api,
                            transport,
                            service_tier: service_tier_policy.selected_tier(),
                        },
                        &error,
                        started.elapsed().as_millis() as u64,
                    );
                    return adapter_error_response(error, Some(&request_id));
                }
                SelectionMiss::Respond(response) => return response,
            }
        };
        detach_encrypted_context_repair_owner(
            &repairs,
            &mut response_affinity_key,
            &mut requires_affinity_owner,
        );
        let Some(route) = admitted.remove(&selected.candidate_id) else {
            preparation_exclusions.insert(selected.candidate_id.clone());
            tried.insert(selected.candidate_id.clone());
            drop(lease);
            continue;
        };
        let driven = drive_selected_attempt(DriveAttemptInput {
            route,
            selected,
            lease,
            request,
            request_id,
            requested_model,
            prompt_affinity_key,
            runtime: &runtime,
            key: &key,
            budget: &budget,
            transport,
            transport_fallback,
            resolved_model: &resolved_model,
            client_wire_api,
            stream,
            responses_lite: &responses_lite,
            service_tier_policy: &service_tier_policy,
            tool_policy: &mut tool_policy,
            client_context_id: &client_context_id,
            last_adapter_error: &mut last_adapter_error,
            forwarded_headers: &forwarded_headers,
            attempt: &mut attempt,
            last_failure: &mut last_failure,
            last_failure_origin: &mut last_failure_origin,
            tried: &mut tried,
            preparation_exclusions: &mut preparation_exclusions,
            has_previous_response_id,
            has_unpaired_tool_output: &mut has_unpaired_tool_output,
            requires_affinity_owner: &mut requires_affinity_owner,
            repairs: &mut repairs,
            last_preserved_upstream_error: &mut last_preserved_upstream_error,
            response_affinity_key: &mut response_affinity_key,
            allow_previous_response_reset,
            confirmed_response_missing: &mut confirmed_response_missing,
        })
        .await;
        let kept = match driven {
            DrivenAttempt::Respond(response) => {
                release_encrypted_context_repair_owner(
                    &mut repairs,
                    &mut response_affinity_key,
                    &mut requires_affinity_owner,
                    &runtime,
                );
                return response;
            }
            DrivenAttempt::Continue(kept) => kept,
            DrivenAttempt::Break(AttemptCarry {
                request: next_request,
                request_id: next_request_id,
                requested_model: next_requested_model,
                prompt_affinity_key: _,
            }) => {
                release_encrypted_context_repair_owner(
                    &mut repairs,
                    &mut response_affinity_key,
                    &mut requires_affinity_owner,
                    &runtime,
                );
                request = next_request;
                request_id = next_request_id;
                requested_model = next_requested_model;
                break;
            }
        };
        request = kept.request;
        request_id = kept.request_id;
        requested_model = kept.requested_model;
        prompt_affinity_key = kept.prompt_affinity_key;
    }

    release_encrypted_context_repair_owner(
        &mut repairs,
        &mut response_affinity_key,
        &mut requires_affinity_owner,
        &runtime,
    );

    if allow_previous_response_reset
        && request_has_previous_response_id(client_wire_api, &request)
        && confirmed_response_missing
    {
        let mut reset_request = request;
        if drop_materialized_previous_response_id(
            &runtime,
            &key.id,
            &mut reset_request,
            &resolved_model,
            now_ms(),
        ) {
            return Box::pin(execute_request(RequestExecution {
                tool_policy,
                runtime,
                key,
                request: reset_request,
                service_tier_policy,
                requested_model,
                resolved_model,
                stream,
                request_id,
                forwarded_headers,
                client_context_id,
                client_affinity_id,
                response_affinity_key: None,
                requires_affinity_owner: false,
                client_wire_api,
                responses_lite,
                allow_previous_response_reset: false,
                attempt_offset: attempt,
                budget,
                transport,
                transport_fallback,
                reserved_candidate: None,
            }))
            .await;
        }
        return api_error(
            StatusCode::CONFLICT,
            RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
            RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
        );
    }

    if last_failure.is_none() {
        if let Some(error) = last_adapter_error {
            record_adapter_rejection(
                &runtime,
                AdapterRejection {
                    request_id: &request_id,
                    attempt,
                    local_key_id: &key.id,
                    requested_model: &requested_model,
                    resolved_model: &resolved_model,
                    client_context_id: client_context_id.as_deref(),
                    wire_api: client_wire_api,
                    transport,
                    service_tier: service_tier_policy.selected_tier(),
                },
                &error,
                started.elapsed().as_millis() as u64,
            );
            return adapter_error_response(error, Some(&request_id));
        }
    }
    if let Some(error) = crate::gateway::errors::admission_error(&budget) {
        return error;
    }
    let failure = AttemptFailure::after_exhausted_attempts(retry_window_expired, last_failure);
    finish_request_failure(RequestFailureInput {
        runtime: &runtime,
        key: &key,
        resolved_model: &resolved_model,
        protocols: candidate_protocols(client_wire_api),
        operation: crate::scheduler::rotation::RotationOperation::Text,
        exclusions: &HashSet::new(),
        response_affinity_key: response_affinity_key.as_deref(),
        failure,
        preserved: last_preserved_upstream_error.as_ref(),
        failure_origin: last_failure_origin,
        request_id: &request_id,
    })
}

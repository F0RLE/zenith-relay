use super::super::prelude::*;
use super::super::recovery::{adapter_error_response, replay_native_tool_continuation};
use super::FailureStep;

pub(super) fn settle_collected_rejection(
    super::CollectedRejection {
        status,
        response_headers,
        runtime,
        lease,
        route,
        source_model,
        request_id,
        key,
        carry:
            super::RejectionCarry {
                client_wire_api,
                request,
                adapter_is_passthrough,
                has_previous_response_id,
                repairs,
                tried,
                has_unpaired_tool_output,
                requires_affinity_owner,
                last_failure,
                last_failure_origin,
                last_preserved_upstream_error,
                stream,
                response_affinity_key,
                response_affinity_hit,
                selected_error_origin,
                prompt_affinity_key,
                confirmed_response_missing,
                account_route,
                forwarded_headers,
                ..
            },
        event,
        bytes,
        ..
    }: super::CollectedRejection<'_>,
    failure: AttemptFailure,
) -> FailureStep {
    let native_replay_attempted = &mut repairs.native_replay;
    let cache_write_rejected = prompt_cache_write_rejected(&bytes);
    let rejection_state =
        settle_attempt_failure(runtime, lease, source_model, &failure, response_headers);
    let response_missing = previous_response_not_found(&bytes);
    let affinity_miss = recoverable_response_affinity_miss(
        status,
        has_previous_response_id,
        response_affinity_hit,
        response_missing,
    );
    // A Responses continuation normally has to stay on the creator
    // of `previous_response_id`.  When that owner is temporarily
    // unavailable, use the volatile native replay captured from the
    // successful turn before selecting another candidate.  This is
    // the safe hand-off path: the new candidate receives the
    // materialized conversation, never a foreign opaque response id.
    if client_wire_api == WireApi::Responses
        && adapter_is_passthrough
        && has_previous_response_id
        && response_affinity_hit
        && *requires_affinity_owner
        && !*native_replay_attempted
        && (retryable_failure(status, failure.category, has_previous_response_id)
            || (affinity_miss && response_missing))
    {
        match replay_native_tool_continuation(
            runtime,
            &key.id,
            request,
            route,
            stream,
            native_replay_attempted,
        ) {
            Ok(true) => {
                clear_materialized_continuation(
                    response_affinity_key,
                    requires_affinity_owner,
                    has_unpaired_tool_output,
                );
                if response_missing {
                    // The owner is healthy but has lost its opaque
                    // response id. It can safely accept the
                    // materialized conversation on the next attempt.
                    tried.remove(&route.candidate_id);
                    lease.allow_rotation_repair();
                    event.error_category = Some(error_codes::RESPONSE_AFFINITY_MISS.to_string());
                } else {
                    let state = rejection_state.clone();
                    apply_failure_state(event, state);
                }
                // A retryable transport/availability failure leaves
                // the owner in `tried`: replay has materialized the
                // conversation specifically so the next attempt can
                // be handed to a different slot.
                emit_usage(runtime, event.clone());
                *last_failure = Some(failure);
                *last_failure_origin = selected_error_origin;
                return FailureStep::Continue;
            }
            Ok(false) => {}
            Err(error) => return FailureStep::Respond(adapter_error_response(error)),
        }
    }
    if cache_write_rejected {
        runtime.invalidate_prompt_affinity(prompt_affinity_key.as_deref());
    }
    if affinity_miss
        || cache_write_rejected
        || retryable_failure(status, failure.category, has_previous_response_id)
    {
        if affinity_miss {
            *confirmed_response_missing |= response_missing;
            runtime.invalidate_response_affinity(response_affinity_key.as_deref());
            event.error_category = Some(error_codes::RESPONSE_AFFINITY_MISS.to_string());
        } else {
            let state = rejection_state.clone();
            apply_failure_state(event, state);
        }
        emit_usage(runtime, event.clone());
        *last_failure = Some(failure);
        *last_failure_origin = selected_error_origin;
        if affinity_miss && response_missing && response_affinity_hit {
            return FailureStep::Break;
        }
        return FailureStep::Continue;
    }
    if !adapter_is_passthrough {
        emit_usage(runtime, event.clone());
        return FailureStep::Respond(attempt_error_response(
            failure,
            last_preserved_upstream_error.as_ref(),
            selected_error_origin,
            request_id,
        ));
    }
    populate_tokens(event, &bytes);
    emit_usage(runtime, event.clone());
    let origin = selected_error_origin.for_category(failure.category);
    let mut response = proxy_error_response(
        status,
        response_headers,
        &bytes,
        origin,
        failure.category,
        Some(request_id),
    );
    if account_route && adapter_is_passthrough {
        relay_account_response_header(forwarded_headers, response_headers, &mut response);
    }
    FailureStep::Respond(response)
}

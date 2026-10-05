use super::super::prelude::*;
use super::super::recovery::{
    adapter_error_response, recover_stale_tool_history, replay_native_tool_continuation,
    try_repair_legacy_responses_call_ids, LegacyCallIdRepair,
};
use super::FailureStep;

pub(super) enum AfterRepair<'a> {
    Step(FailureStep),
    Proceed(super::CollectedRejection<'a>, AttemptFailure),
}

pub(super) fn repair_collected_rejection(
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
                tool_policy,
                stream,
                response_affinity_key,
                resolved_model,
                allow_previous_response_reset,
                response_affinity_hit,
                selected_error_origin,
                prompt_affinity_key,
                confirmed_response_missing,
                account_route,
                forwarded_headers,
            },
        event,
        bytes,
    }: super::CollectedRejection<'_>,
) -> AfterRepair<'_> {
    let AttemptRepairs {
        legacy_call_id: legacy_call_id_repair_attempted,
        function_item_id: function_item_id_repair_attempted,
        custom_tool_item_id: custom_tool_item_id_repair_attempted,
        message_item_id: message_item_id_repair_attempted,
        native_replay: native_replay_attempted,
        stale_tool_history: stale_tool_history_recovered,
        model_switch_reset: model_switch_reset_attempted,
        encrypted_context: encrypted_context_attempted,
        ..
    } = repairs;
    if try_repair_legacy_responses_call_ids(LegacyCallIdRepair {
        request,
        client_wire_api,
        adapter_is_passthrough,
        upstream_rejected_tool_links: status.is_client_error()
            && responses_tool_call_links_rejected(&bytes),
        repair_attempted: legacy_call_id_repair_attempted,
        tried,
        candidate_id: &route.candidate_id,
        has_unpaired_tool_output,
        requires_affinity_owner,
    }) {
        lease.settle_rotation_repair(now_ms());
        return AfterRepair::Step(FailureStep::Continue);
    }
    if client_wire_api == WireApi::Responses
        && adapter_is_passthrough
        && repair_responses_item_prefixes(
            request,
            &bytes,
            true,
            &mut ResponsesItemPrefixRepairs {
                function_ids: function_item_id_repair_attempted,
                custom_tool_ids: custom_tool_item_id_repair_attempted,
                message_ids: message_item_id_repair_attempted,
            },
            tried,
            &route.candidate_id,
            lease,
        )
    {
        lease.settle_rotation_repair(now_ms());
        return AfterRepair::Step(FailureStep::Continue);
    }
    let mut failure = AttemptFailure::status_with_body(status, Some(&bytes));
    super::super::super::super::errors::apply_degraded_route_policy(runtime, &mut failure);
    *last_preserved_upstream_error = preserved_upstream_error(&failure, &bytes);
    let upstream_error =
        crate::usage::UpstreamErrorDetails::from_body(Some(status.as_u16()), &bytes);
    event.upstream_error = Some(upstream_error.clone());
    event.error_category = Some(failure.category.to_string());
    if client_wire_api == WireApi::Responses
        && adapter_is_passthrough
        && has_previous_response_id
        && !*native_replay_attempted
        && (previous_response_requires_websocket(&bytes)
            || (status == StatusCode::BAD_REQUEST
                && contains_tool_call_output(request)
                && (responses_function_call_output_has_invalid_call_id(&bytes)
                    || zenith_gateway_invalid_request(&bytes))))
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
                tried.remove(&route.candidate_id);
                lease.allow_rotation_repair();
                emit_usage(runtime, event.clone());
                *last_failure = Some(failure);
                *last_failure_origin = selected_error_origin;
                lease.settle_rotation_repair(now_ms());
                return AfterRepair::Step(FailureStep::Continue);
            }
            Ok(false) => {}
            Err(error) => {
                return AfterRepair::Step(FailureStep::Respond(adapter_error_response(error)))
            }
        }
    }
    if client_wire_api == WireApi::Responses
        && has_previous_response_id
        && responses_tool_call_is_missing_output(&bytes)
        && recover_stale_tool_history(
            runtime,
            &key.id,
            request,
            resolved_model,
            &bytes,
            stale_tool_history_recovered,
        )
    {
        clear_materialized_continuation(
            response_affinity_key,
            requires_affinity_owner,
            has_unpaired_tool_output,
        );
        tried.remove(&route.candidate_id);
        lease.allow_rotation_repair();
        emit_usage(runtime, event.clone());
        *last_failure = Some(failure);
        *last_failure_origin = selected_error_origin;
        lease.settle_rotation_repair(now_ms());
        return AfterRepair::Step(FailureStep::Continue);
    }
    if client_wire_api == WireApi::Responses
        && allow_previous_response_reset
        && !*model_switch_reset_attempted
        && reset_materialized_continuation(
            &mut ContinuationReset {
                attempted: model_switch_reset_attempted,
                response_affinity_key,
                requires_affinity_owner,
            },
            recoverable_response_model_switch(
                status,
                failure.category,
                has_previous_response_id,
                *has_unpaired_tool_output,
                &bytes,
            ),
            runtime,
            &key.id,
            request,
            resolved_model,
        )
    {
        emit_usage(runtime, event.clone());
        *last_failure = Some(failure);
        *last_failure_origin = selected_error_origin;
        lease.settle_rotation_repair(now_ms());
        return AfterRepair::Step(FailureStep::Continue);
    }
    // Keep encrypted context intact for the first attempt. After a ChatGPT
    // account explicitly rejects it, remove ciphertext-bearing Responses
    // history and permit one pre-output retry; visible summaries remain.
    if client_wire_api == WireApi::Responses
        && route.account_id.is_some()
        && super::super::super::repair_once(
            encrypted_context_attempted,
            failure.category == error_codes::UPSTREAM_ENCRYPTED_CONTENT_INVALID,
            tried,
            &route.candidate_id,
            lease,
            || super::super::super::account::drop_rejected_encrypted_context(request),
        )
    {
        emit_usage(runtime, event.clone());
        *last_failure = Some(failure);
        *last_failure_origin = selected_error_origin;
        lease.settle_rotation_repair(now_ms());
        return AfterRepair::Step(FailureStep::Continue);
    }
    AfterRepair::Proceed(
        super::CollectedRejection {
            status,
            response_headers,
            runtime,
            lease,
            route,
            source_model,
            request_id,
            key,
            carry: super::RejectionCarry {
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
                tool_policy,
                stream,
                response_affinity_key,
                resolved_model,
                allow_previous_response_reset,
                response_affinity_hit,
                selected_error_origin,
                prompt_affinity_key,
                confirmed_response_missing,
                account_route,
                forwarded_headers,
            },
            event,
            bytes,
        },
        failure,
    )
}

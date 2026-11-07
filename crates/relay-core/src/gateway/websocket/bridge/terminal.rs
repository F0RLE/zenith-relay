use super::super::*;
use super::BridgeState;

pub(super) fn finish_terminal(
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    terminal: EventTerminal,
) -> bool {
    let Some(outcome) = terminal.outcome else {
        return true;
    };
    let Some(mut in_flight) = state.in_flight.take() else {
        return true;
    };
    // A scoped terminal settles only this owner, including the default lane.
    // Incomplete responses retain only a live-connection owner binding;
    // durable affinity and successful health still require completion.
    let terminal_success = matches!(outcome, EventTerminalOutcome::Success);
    in_flight.event.latency_ms = in_flight.started.elapsed().as_millis() as u64;
    in_flight.event.generation_ms = in_flight
        .event
        .ttft_ms
        .map(|ttft_ms| in_flight.event.latency_ms.saturating_sub(ttft_ms))
        .filter(|duration| *duration > 0);
    in_flight.event.success = terminal_success;
    if let Some(routing) = in_flight.event.routing.as_mut() {
        routing
            .attempt_outcome
            .get_or_insert(crate::AttemptOutcome::ReturnedFailure);
    }
    if terminal_success {
        in_flight.event.tool_use.finish();
    }
    if matches!(outcome, EventTerminalOutcome::Incomplete) {
        in_flight.event.error_category = Some(error_codes::RESPONSE_INCOMPLETE.to_string());
    }
    if terminal.deactivated_workspace
        && terminal.status == Some(StatusCode::PAYMENT_REQUIRED)
        && in_flight.route.account_id.is_some()
    {
        runtime.trip_chatgpt_team_breaker(&in_flight.route.candidate_id, now_ms());
    }
    runtime.observe_codex_quota_headers(
        &in_flight.route.candidate_id,
        in_flight.route.account_token_generation,
        in_flight.route.account_token_revision.as_ref(),
        match outcome {
            EventTerminalOutcome::Success => terminal.status.unwrap_or(StatusCode::OK),
            EventTerminalOutcome::Incomplete => StatusCode::OK,
            EventTerminalOutcome::Failure => terminal.status.unwrap_or(StatusCode::BAD_GATEWAY),
        },
        &terminal.headers,
        now_ms(),
    );
    // A continuation may legitimately reference an incomplete response (for
    // example after max_output_tokens). Keep that id only for this live
    // WebSocket; durable response/prompt affinity still requires success.
    if terminal_success || matches!(outcome, EventTerminalOutcome::Incomplete) {
        let transient_affinity = (!terminal_success)
            .then(|| {
                runtime.bind_volatile_response_affinity(
                    in_flight.response_id.as_deref(),
                    &in_flight.route.candidate_id,
                    &in_flight.request.request_id,
                    now_ms(),
                )
            })
            .flatten();
        let retired = state.responses.record(
            in_flight.request.stream_id.as_deref(),
            in_flight.response_id.as_deref(),
            transient_affinity,
        );
        runtime.invalidate_response_affinity(retired.as_deref());
    } else {
        let retired = state.responses.retire_failed_continuation(
            in_flight.request.stream_id.as_deref(),
            in_flight.request.previous_response_id(),
        );
        runtime.invalidate_response_affinity(retired.as_deref());
    }
    {
        let lease = in_flight.lease;
        if terminal_success {
            let recovered =
                lease.settle_rotation_success_with_latency(now_ms(), in_flight.event.ttft_ms);
            in_flight.event.consecutive_failures = recovered.then_some(0);
        } else if outcome != EventTerminalOutcome::Failure {
            lease.settle_rotation_terminal(now_ms());
        } else {
            let mut failure = super::super::super::errors::AttemptFailure::classified_with_hint(
                terminal_failure_status(terminal.status),
                terminal
                    .error_category
                    .unwrap_or(error_codes::UPSTREAM_TERMINAL),
                terminal.body_hint,
            );
            if in_flight.client_visible_output {
                failure.execution = crate::scheduler::rotation::ExecutionObservation::committed();
            } else if terminal.generation_evidence {
                failure.execution = crate::scheduler::rotation::ExecutionObservation::accepted();
            }
            super::super::super::errors::settle_attempt_failure(
                runtime,
                &lease,
                &in_flight.route.source_model,
                &failure,
                &terminal.headers,
            );
        }
    }
    if terminal_success {
        in_flight
            .tool_call_capture
            .bind(runtime, &state.local_key_id, &in_flight.route, now_ms());
        if let Some(response) = in_flight
            .native_replay_capture
            .finish(terminal.response.clone(), in_flight.response_id.as_deref())
        {
            runtime.capture_native_responses_replay(
                &state.local_key_id,
                &in_flight.route.candidate_id,
                &in_flight.request.native_replay_request(),
                &in_flight.route.source_model,
                &response,
                now_ms(),
            );
        }
        runtime.bind_response_route_affinity(
            in_flight.response_id.as_deref(),
            &in_flight.route,
            now_ms(),
        );
        runtime.bind_prompt_affinity(
            in_flight.prompt_affinity_key.as_deref(),
            &in_flight.route.candidate_id,
            now_ms(),
        );
    } else if matches!(outcome, EventTerminalOutcome::Failure) {
        let (status, category) = resolved_terminal_failure(&terminal);
        let status = super::super::super::errors::canonical_upstream_status(status, category);
        in_flight.event.http_status = status.as_u16();
        in_flight.event.error_category = Some(category.to_string());
        in_flight.event.upstream_error = terminal.upstream_error;
        if super::super::super::errors::retryable_failure(status, category, false) {
            let failure_state = current_failure_state(
                runtime,
                &in_flight.route.candidate_id,
                &in_flight.route.source_model,
            );
            apply_failure_state(&mut in_flight.event, failure_state);
        }
    }
    emit_usage(runtime, in_flight.event);
    true
}

pub(super) fn finish_incomplete(runtime: &GatewayRuntime, state: &mut BridgeState, category: &str) {
    state.pending_admission.take();
    clear_connection_responses(runtime, state);
    while state.focus_remaining_request() {
        finish_incomplete_request(runtime, state, category);
    }
}

fn finish_incomplete_request(runtime: &GatewayRuntime, state: &mut BridgeState, category: &str) {
    let Some(mut in_flight) = state.in_flight.take() else {
        return;
    };
    in_flight.lease.settle_rotation_unknown(now_ms());
    drop(in_flight.lease);
    in_flight.event.success = false;
    in_flight.event.error_category = Some(category.to_string());
    if let Some(routing) = in_flight.event.routing.as_mut() {
        routing.attempt_outcome = Some(
            if matches!(category, error_codes::CLIENT_CANCELLED | "client_websocket") {
                crate::AttemptOutcome::ClientDisconnected
            } else {
                crate::AttemptOutcome::ExecutionUncertain
            },
        );
    }
    in_flight.event.latency_ms = in_flight.started.elapsed().as_millis() as u64;
    if let Some(status) = incomplete_status(category) {
        in_flight.event.http_status = status.as_u16();
    }
    in_flight.event.generation_ms = in_flight
        .event
        .ttft_ms
        .map(|ttft_ms| in_flight.event.latency_ms.saturating_sub(ttft_ms))
        .filter(|duration| *duration > 0);
    // Cool only this physical slot and model. A stream failure on one
    // credential must not make unrelated routes unavailable, regardless of
    // whether the selected route is an OAuth account or a direct API source.
    if incomplete_requires_cooldown(category)
        && !matches!(
            category,
            error_codes::UPSTREAM_WEBSOCKET_CLOSED | error_codes::UPSTREAM_WEBSOCKET
        )
    {
        let failure_state = current_failure_state(
            runtime,
            &in_flight.route.candidate_id,
            &in_flight.route.source_model,
        );
        apply_failure_state(&mut in_flight.event, failure_state);
    }
    emit_usage(runtime, in_flight.event);
}

pub(super) fn clear_connection_responses(runtime: &GatewayRuntime, state: &mut BridgeState) {
    for key in state.responses.clear() {
        runtime.invalidate_response_affinity(Some(&key));
    }
}

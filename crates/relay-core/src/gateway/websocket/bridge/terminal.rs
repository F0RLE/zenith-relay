use super::super::upstream::first_message_terminal;
use super::super::*;
use super::BridgeState;

pub(super) fn retryable_disconnect_request(
    runtime: &GatewayRuntime,
    state: &BridgeState,
) -> Option<ClientRequest> {
    let in_flight = state.in_flight.as_ref()?;
    if in_flight.request.budget.dispatches() > 0 {
        // A disconnect after response.create has no pre-execution proof.
        return None;
    }
    if in_flight.client_visible_output
        || in_flight.event.ttft_ms.is_some()
        || in_flight.event.output_tokens.is_some()
        || in_flight.event.tool_use.tool_call_count > 0
        || in_flight.event.tool_use.text_output
    {
        return None;
    }
    if in_flight.request.has_previous_response_id() {
        return replay_in_flight_continuation(runtime, state, false);
    }
    if in_flight.request.has_unpaired_tool_output() {
        return None;
    }
    Some(in_flight.request.clone())
}

fn replay_in_flight_continuation(
    runtime: &GatewayRuntime,
    state: &BridgeState,
    retain_owner: bool,
) -> Option<ClientRequest> {
    let in_flight = state.in_flight.as_ref()?;
    let mut request = in_flight.request.clone();
    let owner_key = request.response_affinity_key.clone();
    request
        .replay_native_continuation(
            runtime,
            &state.local_key_id,
            &in_flight.route.candidate_id,
            &in_flight.route.source_model,
        )
        .ok()?
        .then_some(request)
        .map(|mut request| {
            if retain_owner {
                request.response_affinity_key = owner_key;
                crate::gateway::continuation::retain_materialized_continuation_owner(
                    &mut request.requires_affinity_owner,
                    &mut request.has_unpaired_tool_output,
                );
            }
            request
        })
}

pub(super) fn retryable_terminal_request(
    runtime: &GatewayRuntime,
    state: &BridgeState,
    message: &UpstreamMessage,
) -> Option<ClientRequest> {
    let terminal = first_message_terminal(message)?;
    if terminal.outcome != Some(EventTerminalOutcome::Failure) {
        return None;
    }
    let in_flight = state.in_flight.as_ref()?;
    if in_flight.client_visible_output
        || in_flight.event.ttft_ms.is_some()
        || in_flight.event.output_tokens.is_some()
        || in_flight.event.tool_use.tool_call_count > 0
        || in_flight.event.tool_use.text_output
    {
        return None;
    }
    let (status, category) = resolved_terminal_failure(&terminal);
    let status = super::super::super::errors::canonical_upstream_status(status, category);
    if in_flight.request.has_previous_response_id() {
        return super::super::super::errors::retryable_failure(status, category, true)
            .then(|| replay_in_flight_continuation(runtime, state, false))
            .flatten();
    }
    if in_flight.request.has_unpaired_tool_output()
        || !super::super::super::errors::retryable_failure(status, category, false)
    {
        return None;
    }
    Some(in_flight.request.clone())
}

pub(super) fn repairable_terminal_request(
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    message: &UpstreamMessage,
) -> Option<ClientRequest> {
    let terminal = first_message_terminal(message)?;
    if terminal.outcome != Some(EventTerminalOutcome::Failure) {
        return None;
    }
    if terminal.previous_response_not_found {
        let in_flight = state.in_flight.as_ref()?;
        if in_flight.client_visible_output {
            return None;
        }
        let request = replay_in_flight_continuation(runtime, state, true)?;
        return Some(request);
    }
    let body = match message {
        UpstreamMessage::Text(text) => Some(text.as_bytes()),
        UpstreamMessage::Binary(bytes) => Some(bytes.as_ref()),
        _ => None,
    }?;
    if !super::super::super::errors::responses_tool_call_links_rejected(body) {
        return None;
    }
    let in_flight = state.in_flight.as_mut()?;
    if in_flight.client_visible_output || in_flight.legacy_call_id_repair_attempted {
        return None;
    }
    let mut request = in_flight.request.clone();
    if !request.repair_legacy_call_ids() {
        return None;
    }
    in_flight.legacy_call_id_repair_attempted = true;
    Some(request)
}

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
    // `response.incomplete` is a terminal event for this request, so the
    // client WebSocket may carry its next independent request. It is not a
    // successful response: do not retain response/session affinity or reset
    // slot health from a partially completed stream.
    let terminal_success = matches!(outcome, EventTerminalOutcome::Success);
    let keep_client_socket = !matches!(outcome, EventTerminalOutcome::Failure);
    in_flight.event.latency_ms = in_flight.started.elapsed().as_millis() as u64;
    in_flight.event.generation_ms = in_flight
        .event
        .ttft_ms
        .map(|ttft_ms| in_flight.event.latency_ms.saturating_sub(ttft_ms))
        .filter(|duration| *duration > 0);
    in_flight.event.success = terminal_success;
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
    if terminal_success {
        clear_transient_response_affinity(runtime, state);
        state.last_response_id = in_flight.response_id.clone();
    } else if matches!(outcome, EventTerminalOutcome::Incomplete) {
        clear_transient_response_affinity(runtime, state);
        state.last_response_id = in_flight.response_id.clone();
        state.transient_response_affinity_key = runtime.bind_volatile_response_affinity(
            in_flight.response_id.as_deref(),
            &in_flight.route.candidate_id,
            &in_flight.request.request_id,
            now_ms(),
        );
    }
    if let Some(lease) = state.lease.take() {
        if terminal_success {
            lease.settle_rotation_success(now_ms());
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
        if let Some(response) = in_flight
            .native_replay_capture
            .finish(terminal.response.clone(), in_flight.response_id.as_deref())
        {
            runtime.capture_native_responses_replay(
                &state.local_key_id,
                &in_flight.route.candidate_id,
                &in_flight.request.native_replay_value(),
                &in_flight.route.source_model,
                &response,
                now_ms(),
            );
        }
        let recovered = runtime.record_success_with_metrics(
            &in_flight.route.candidate_id,
            &in_flight.route.source_model,
            now_ms(),
            in_flight.event.output_tokens,
            in_flight
                .event
                .generation_ms
                .unwrap_or(in_flight.event.latency_ms),
        );
        runtime.bind_response_affinity(
            in_flight.response_id.as_deref(),
            &in_flight.route.candidate_id,
            now_ms(),
        );
        runtime.bind_prompt_affinity(
            in_flight.prompt_affinity_key.as_deref(),
            &in_flight.route.candidate_id,
            now_ms(),
        );
        in_flight.event.consecutive_failures = recovered.then_some(0);
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
    keep_client_socket
}

pub(super) fn finish_incomplete(runtime: &GatewayRuntime, state: &mut BridgeState, category: &str) {
    if let Some(lease) = state.lease.take() {
        lease.settle_rotation_unknown(now_ms());
    }
    clear_transient_response_affinity(runtime, state);
    let Some(mut in_flight) = state.in_flight.take() else {
        state.lease.take();
        return;
    };
    in_flight.event.success = false;
    in_flight.event.error_category = Some(category.to_string());
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
    state.lease.take();
}

pub(super) fn clear_transient_response_affinity(runtime: &GatewayRuntime, state: &mut BridgeState) {
    if let Some(key) = state.transient_response_affinity_key.take() {
        runtime.invalidate_response_affinity(Some(&key));
    }
}

use super::super::*;
use super::terminal::{finish_incomplete, finish_terminal};
use super::BridgeState;

pub(super) async fn handle_initial_messages(
    downstream: &mut WebSocket,
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    initial_messages: Vec<UpstreamMessage>,
) -> bool {
    for message in initial_messages {
        if !handle_upstream_message(downstream, runtime, state, message).await {
            return false;
        }
    }
    true
}

pub(super) async fn handle_upstream_message(
    downstream: &mut WebSocket,
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    message: UpstreamMessage,
) -> bool {
    if runtime.block_degraded_routes_enabled()
        && state.in_flight.as_ref().is_some_and(|in_flight| {
            in_flight.route.account_id.is_some()
                && super::super::upstream::message_serves_rejected_model(
                    &message,
                    &in_flight.route.source_model,
                )
        })
    {
        let request_id = state.request_id().map(str::to_owned);
        let stream_id = state.request_stream_id().map(str::to_owned);
        // Keep actual usage even though this frame will not be forwarded.
        match &message {
            UpstreamMessage::Text(text) => {
                inspect_upstream_event(text.as_bytes(), state);
            }
            UpstreamMessage::Binary(bytes) => {
                inspect_upstream_event(bytes, state);
            }
            _ => {}
        }
        // This bridge may already have forwarded setup or output bytes. End
        // this response and retain its owner; do not reconnect and replay it.
        if let Some(in_flight) = state.in_flight.as_mut() {
            in_flight.client_visible_output = true;
        }
        finish_terminal(
            runtime,
            state,
            EventTerminal {
                outcome: Some(EventTerminalOutcome::Failure),
                status: Some(StatusCode::NOT_FOUND),
                error_category: Some(error_codes::UPSTREAM_ROUTE_DEGRADED),
                ..EventTerminal::default()
            },
        );
        send_gateway_error(
            downstream,
            &GatewayFailure::classified(
                StatusCode::NOT_FOUND,
                error_codes::UPSTREAM_ROUTE_DEGRADED,
                state.upstream_origin,
            ),
            request_id.as_deref(),
            stream_id.as_deref(),
        )
        .await;
        return false;
    }
    match message {
        UpstreamMessage::Text(text) => {
            if text.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
                reject_oversized_upstream_message(downstream, runtime, state).await;
                return false;
            }
            let Some((terminal, visible)) = prepare_upstream_forward(text.as_bytes(), state) else {
                return true;
            };
            if visible {
                if let Some(in_flight) = state.in_flight.as_mut() {
                    in_flight.client_visible_output = true;
                }
            }
            let outgoing =
                prefix_websocket_error_payload(text.as_bytes(), &terminal, state.upstream_origin);
            let outgoing = String::from_utf8(outgoing).unwrap_or_else(|_| text.to_string());
            if downstream
                .send(Message::Text(outgoing.into()))
                .await
                .is_err()
            {
                finish_incomplete(runtime, state, error_codes::CLIENT_CANCELLED);
                return false;
            }
            finish_terminal(runtime, state, terminal)
        }
        UpstreamMessage::Binary(bytes) => {
            if bytes.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
                reject_oversized_upstream_message(downstream, runtime, state).await;
                return false;
            }
            let Some((terminal, visible)) = prepare_upstream_forward(bytes.as_ref(), state) else {
                return true;
            };
            if visible {
                if let Some(in_flight) = state.in_flight.as_mut() {
                    in_flight.client_visible_output = true;
                }
            }
            let outgoing =
                prefix_websocket_error_payload(bytes.as_ref(), &terminal, state.upstream_origin);
            if downstream
                .send(Message::Binary(outgoing.into()))
                .await
                .is_err()
            {
                finish_incomplete(runtime, state, error_codes::CLIENT_CANCELLED);
                return false;
            }
            finish_terminal(runtime, state, terminal)
        }
        UpstreamMessage::Ping(payload) => downstream.send(Message::Ping(payload)).await.is_ok(),
        UpstreamMessage::Pong(payload) => downstream.send(Message::Pong(payload)).await.is_ok(),
        UpstreamMessage::Close { code, reason } => {
            let active_request = state.in_flight.is_some() && state.can_send_gateway_error();
            let request_id = state.request_id().map(str::to_owned);
            let stream_id = state.request_stream_id().map(str::to_owned);
            finish_incomplete(runtime, state, error_codes::UPSTREAM_WEBSOCKET_CLOSED);
            if active_request {
                send_gateway_error(
                    downstream,
                    &GatewayFailure::closed(state.upstream_origin),
                    request_id.as_deref(),
                    stream_id.as_deref(),
                )
                .await;
            } else {
                let _ = downstream
                    .send(Message::Close(Some(CloseFrame {
                        code: u16::from(code),
                        reason: reason.into(),
                    })))
                    .await;
            }
            false
        }
    }
}

fn prefix_websocket_error_payload(
    payload: &[u8],
    terminal: &EventTerminal,
    origin: crate::ErrorOrigin,
) -> Vec<u8> {
    if !matches!(
        terminal.outcome,
        Some(EventTerminalOutcome::Failure | EventTerminalOutcome::Incomplete)
    ) {
        return payload.to_vec();
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(payload) else {
        return payload.to_vec();
    };
    let category = terminal
        .error_category
        .unwrap_or(error_codes::UPSTREAM_TERMINAL);
    let origin = origin.for_category(category);
    if !super::super::super::errors::prefix_error_value(&mut value, origin) {
        return payload.to_vec();
    }
    serde_json::to_vec(&value).unwrap_or_else(|_| payload.to_vec())
}

async fn reject_oversized_upstream_message(
    downstream: &mut WebSocket,
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
) {
    let request_id = state.request_id().map(str::to_owned);
    let stream_id = state.request_stream_id().map(str::to_owned);
    let can_send_error = state.can_send_gateway_error();
    finish_incomplete(runtime, state, error_codes::STREAM_EVENT_TOO_LARGE);
    if can_send_error {
        send_gateway_error(
            downstream,
            &GatewayFailure::message_too_large(state.upstream_origin),
            request_id.as_deref(),
            stream_id.as_deref(),
        )
        .await;
    }
}

fn is_response_frame(payload: &[u8]) -> bool {
    serde_json::from_slice::<Value>(payload)
        .ok()
        .is_some_and(|value| {
            value
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|event_type| event_type.starts_with("response."))
        })
}

/// A completed turn no longer owns upstream response frames. Keep late
/// `response.*` events off the next client turn.
fn prepare_upstream_forward(
    payload: &[u8],
    state: &mut BridgeState,
) -> Option<(EventTerminal, bool)> {
    if state.in_flight.is_none() && is_late_response_frame(payload) {
        return None;
    }
    Some(classify_upstream_payload(payload, state))
}

fn is_late_response_frame(payload: &[u8]) -> bool {
    crate::gateway::streaming::fast_response_delta_json(payload).is_some()
        || is_response_frame(payload)
}

fn classify_upstream_payload(payload: &[u8], state: &mut BridgeState) -> (EventTerminal, bool) {
    if let Some(delta) = crate::gateway::streaming::fast_response_delta_json(payload) {
        record_fast_response_delta(state, delta);
        return (EventTerminal::default(), true);
    }
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        if let Some(in_flight) = state.in_flight.as_mut() {
            in_flight.native_replay_capture.mark_unmaterialized();
        }
        // Malformed frames are conservative visible output: a pre-output
        // reconnect must not replay bytes the client may already have seen.
        return (EventTerminal::default(), true);
    };
    (
        inspect_parsed_event(&value, state),
        semantic_output_value(&value),
    )
}

fn record_fast_response_delta(
    state: &mut BridgeState,
    delta: crate::gateway::streaming::FastResponseDelta,
) {
    let Some(in_flight) = state.in_flight.as_mut() else {
        return;
    };
    in_flight
        .native_replay_capture
        .observe_response_delta(delta.output_index);
    if delta.nonempty_text && in_flight.event.ttft_ms.is_none() {
        in_flight.event.ttft_ms = Some(in_flight.started.elapsed().as_millis() as u64);
    }
}

pub(in crate::gateway::websocket) fn semantic_output_payload(payload: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return true;
    };
    semantic_output_value(&value)
}

fn semantic_output_value(value: &Value) -> bool {
    let event_type = value.get("type").and_then(Value::as_str);
    if has_semantic_output(value, event_type) {
        return true;
    }
    // Compaction and known lifecycle notifications are setup/state. A future
    // event is safer to classify as visible than to replay it to a client.
    !is_known_non_output_event(value, event_type)
}

pub(super) fn inspect_upstream_event(payload: &[u8], state: &mut BridgeState) -> EventTerminal {
    classify_upstream_payload(payload, state).0
}

fn inspect_parsed_event(value: &Value, state: &mut BridgeState) -> EventTerminal {
    let event_type = value.get("type").and_then(Value::as_str);
    if let Some(in_flight) = state.in_flight.as_mut() {
        in_flight.native_replay_capture.observe(value);
        in_flight.event.tool_use.observe_stream_payload(value);
        if has_output_delta(value, event_type) && in_flight.event.ttft_ms.is_none() {
            in_flight.event.ttft_ms = Some(in_flight.started.elapsed().as_millis() as u64);
        }
        if let Some(usage) = super::super::super::response::find_usage(value) {
            apply_usage(&mut in_flight.event, usage);
        }
        if let Some(response_id) = super::super::super::response::response_id(value) {
            in_flight.response_id = Some(response_id.to_string());
        }
    }
    event_terminal(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstream_websocket_error_prefixes_the_selected_source() {
        let terminal = EventTerminal {
            outcome: Some(EventTerminalOutcome::Failure),
            error_category: Some(error_codes::UPSTREAM_TERMINAL),
            ..EventTerminal::default()
        };
        let original = br#"{"type":"response.failed","response":{"error":{"code":"server_error","message":"Provider: unavailable"}}}"#;
        let output =
            prefix_websocket_error_payload(original, &terminal, crate::ErrorOrigin::Account);
        let value: Value = serde_json::from_slice(&output).unwrap();

        assert_eq!(
            value["response"]["error"]["message"],
            "Account: unavailable"
        );
        assert_eq!(value["response"]["error"]["code"], "server_error");
    }
}

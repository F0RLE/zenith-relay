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
    match message {
        UpstreamMessage::Text(text) => {
            if text.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
                reject_oversized_upstream_message(downstream, runtime, state).await;
                return false;
            }
            if state.in_flight.is_none() && is_response_frame(text.as_bytes()) {
                // A completed turn no longer owns upstream response frames.
                // Do not leak late deltas/terminals into the next client turn.
                return true;
            }
            let terminal = inspect_upstream_event(text.as_bytes(), state);
            mark_client_visible_output(state, text.as_bytes());
            if downstream.send(Message::Text(text.into())).await.is_err() {
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
            if state.in_flight.is_none() && is_response_frame(&bytes) {
                return true;
            }
            let terminal = inspect_upstream_event(&bytes, state);
            mark_client_visible_output(state, bytes.as_ref());
            if downstream.send(Message::Binary(bytes)).await.is_err() {
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

/// Mark a request as owned by the selected route only after the upstream has
/// emitted semantic response output. Lifecycle/setup events such as
/// `response.created` are still forwarded to the client, but they do not make
/// a pre-output reconnect unsafe. Unknown or malformed frames remain
/// conservative and count as visible output.
fn mark_client_visible_output(state: &mut BridgeState, payload: &[u8]) {
    if semantic_output_payload(payload) {
        if let Some(in_flight) = state.in_flight.as_mut() {
            in_flight.client_visible_output = true;
        }
    }
}

pub(in crate::gateway::websocket) fn semantic_output_payload(payload: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return true;
    };
    let event_type = value.get("type").and_then(Value::as_str);
    if has_semantic_output(&value, event_type) {
        return true;
    }
    // Compaction and known lifecycle notifications are setup/state. A future
    // event is safer to classify as visible than to replay it to a client.
    !is_known_non_output_event(&value, event_type)
}

pub(super) fn inspect_upstream_event(payload: &[u8], state: &mut BridgeState) -> EventTerminal {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        if let Some(in_flight) = state.in_flight.as_mut() {
            in_flight.native_replay_capture.mark_unmaterialized();
        }
        return EventTerminal::default();
    };
    let event_type = value.get("type").and_then(Value::as_str);
    if let Some(in_flight) = state.in_flight.as_mut() {
        in_flight.native_replay_capture.observe(&value);
        in_flight.event.tool_use.observe_stream_payload(&value);
        if has_output_delta(&value, event_type) && in_flight.event.ttft_ms.is_none() {
            in_flight.event.ttft_ms = Some(in_flight.started.elapsed().as_millis() as u64);
        }
        if let Some(usage) = super::super::super::response::find_usage(&value) {
            apply_usage(&mut in_flight.event, usage);
        }
        if let Some(response_id) = super::super::super::response::response_id(&value) {
            in_flight.response_id = Some(response_id.to_string());
        }
    }
    event_terminal(&value)
}

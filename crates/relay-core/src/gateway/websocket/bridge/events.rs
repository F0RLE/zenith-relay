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
            let Some((terminal, has_visible_output)) =
                prepare_upstream_forward(text.as_bytes(), state)
            else {
                return true;
            };
            if has_visible_output {
                if let Some(in_flight) = state.in_flight.as_mut() {
                    in_flight.client_visible_output = true;
                }
            }
            let forwarded_text =
                prefix_websocket_error_payload(text.as_bytes(), &terminal, state.upstream_origin)
                    .and_then(|payload| String::from_utf8(payload).ok());
            let forwarded_message = match forwarded_text {
                Some(rewritten) => Message::Text(rewritten.into()),
                None => Message::Text(text.into()),
            };
            if send_message(downstream, forwarded_message).await.is_err() {
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
            let Some((terminal, has_visible_output)) =
                prepare_upstream_forward(bytes.as_ref(), state)
            else {
                return true;
            };
            if has_visible_output {
                if let Some(in_flight) = state.in_flight.as_mut() {
                    in_flight.client_visible_output = true;
                }
            }
            let forwarded_payload =
                prefix_websocket_error_payload(bytes.as_ref(), &terminal, state.upstream_origin)
                    .map_or_else(|| bytes, Into::into);
            if send_message(downstream, Message::Binary(forwarded_payload))
                .await
                .is_err()
            {
                finish_incomplete(runtime, state, error_codes::CLIENT_CANCELLED);
                return false;
            }
            finish_terminal(runtime, state, terminal)
        }
        UpstreamMessage::Ping(ping_payload) => {
            send_message(downstream, Message::Ping(ping_payload))
                .await
                .is_ok()
        }
        UpstreamMessage::Pong(pong_payload) => {
            send_message(downstream, Message::Pong(pong_payload))
                .await
                .is_ok()
        }
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
                let _ = send_message(
                    downstream,
                    Message::Close(Some(CloseFrame {
                        code: u16::from(code),
                        reason: reason.into(),
                    })),
                )
                .await;
            }
            false
        }
    }
}

/// Returns a replacement only when an error envelope actually needs rewriting.
fn prefix_websocket_error_payload(
    upstream_frame: &[u8],
    terminal: &EventTerminal,
    origin: crate::ErrorOrigin,
) -> Option<Vec<u8>> {
    if !matches!(
        terminal.outcome,
        Some(EventTerminalOutcome::Failure | EventTerminalOutcome::Incomplete)
    ) {
        return None;
    }
    let mut event_payload = serde_json::from_slice::<Value>(upstream_frame).ok()?;
    let category = terminal
        .error_category
        .unwrap_or(error_codes::UPSTREAM_TERMINAL);
    let origin = origin.for_category(category);
    if !super::super::super::errors::prefix_error_value(&mut event_payload, origin) {
        return None;
    }
    serde_json::to_vec(&event_payload).ok()
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

fn is_response_frame(upstream_frame: &[u8]) -> bool {
    serde_json::from_slice::<Value>(upstream_frame)
        .ok()
        .is_some_and(|event_payload| {
            event_payload
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|event_type| event_type.starts_with("response."))
        })
}

/// A completed turn no longer owns upstream response frames. Keep late
/// `response.*` events off the next client turn.
fn prepare_upstream_forward(
    upstream_frame: &[u8],
    state: &mut BridgeState,
) -> Option<(EventTerminal, bool)> {
    if state.in_flight.is_none() && is_late_response_frame(upstream_frame) {
        return None;
    }
    Some(classify_upstream_payload(upstream_frame, state))
}

fn is_late_response_frame(upstream_frame: &[u8]) -> bool {
    crate::gateway::streaming::fast_response_delta_json(upstream_frame).is_some()
        || is_response_frame(upstream_frame)
}

fn classify_upstream_payload(
    upstream_frame: &[u8],
    state: &mut BridgeState,
) -> (EventTerminal, bool) {
    if let Some(delta) = crate::gateway::streaming::fast_response_delta_json(upstream_frame) {
        record_fast_response_delta(state, delta);
        return (EventTerminal::default(), true);
    }
    let Ok(event_payload) = serde_json::from_slice::<Value>(upstream_frame) else {
        if let Some(in_flight) = state.in_flight.as_mut() {
            in_flight.native_replay_capture.mark_unmaterialized();
        }
        // Malformed frames are conservative visible output: a pre-output
        // reconnect must not replay bytes the client may already have seen.
        return (EventTerminal::default(), true);
    };
    (
        inspect_parsed_event(&event_payload, state),
        semantic_output_value(&event_payload),
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

pub(in crate::gateway::websocket) fn semantic_output_payload(upstream_frame: &[u8]) -> bool {
    let Ok(event_payload) = serde_json::from_slice::<Value>(upstream_frame) else {
        return true;
    };
    semantic_output_value(&event_payload)
}

pub(in crate::gateway::websocket) fn semantic_output_value(event_payload: &Value) -> bool {
    let event_type = event_payload.get("type").and_then(Value::as_str);
    if has_semantic_output(event_payload, event_type) {
        return true;
    }
    // Compaction and known lifecycle notifications are setup/state. A future
    // event is safer to classify as visible than to replay it to a client.
    !is_known_non_output_event(event_payload, event_type)
}

pub(super) fn inspect_upstream_event(
    upstream_frame: &[u8],
    state: &mut BridgeState,
) -> EventTerminal {
    classify_upstream_payload(upstream_frame, state).0
}

fn inspect_parsed_event(event_payload: &Value, state: &mut BridgeState) -> EventTerminal {
    let event_type = event_payload.get("type").and_then(Value::as_str);
    if let Some(in_flight) = state.in_flight.as_mut() {
        if in_flight.route.account_id.is_some()
            && in_flight.route.account_transport
                == crate::runtime::AccountTransport::NativeResponses
            && crate::runtime::catalog_hints::native_codex_responses_url(
                &in_flight.route.upstream_url,
            )
        {
            if let Some(transport) = in_flight
                .event
                .routing
                .as_mut()
                .and_then(|routing| routing.transport.as_mut())
            {
                transport
                    .observe_codex_server_timing(event_payload, in_flight.response_id.as_deref());
            }
        }
        // A timing notification is not a response event. In particular, a late
        // notification must never replace the active response's identity.
        if event_type == Some("responsesapi.websocket_timing") {
            return EventTerminal::default();
        }
        in_flight.event.observe_response_metadata(event_payload);
        in_flight.native_replay_capture.observe(event_payload);
        in_flight
            .tool_call_capture
            .observe(event_payload, WireApi::Responses);
        in_flight
            .event
            .tool_use
            .observe_stream_payload(event_payload);
        if has_output_delta(event_payload, event_type) && in_flight.event.ttft_ms.is_none() {
            in_flight.event.ttft_ms = Some(in_flight.started.elapsed().as_millis() as u64);
        }
        if let Some(usage) = super::super::super::response::find_usage(event_payload) {
            crate::protocol::merge_usage(&mut in_flight.usage_snapshot, usage);
            if let Some(snapshot) = in_flight.usage_snapshot.as_ref() {
                apply_usage(&mut in_flight.event, snapshot);
            }
        }
        if let Some(response_id) = super::super::super::response::response_id(event_payload) {
            in_flight.response_id = Some(response_id.to_string());
        }
    }
    event_terminal(event_payload)
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
        let prefixed_payload =
            prefix_websocket_error_payload(original, &terminal, crate::ErrorOrigin::Account)
                .unwrap();
        let error_response: Value = serde_json::from_slice(&prefixed_payload).unwrap();

        assert_eq!(
            error_response["response"]["error"]["message"],
            "Account: unavailable"
        );
        assert_eq!(error_response["response"]["error"]["code"], "server_error");
    }

    #[test]
    fn ordinary_frames_do_not_build_replacement_payloads() {
        let original = br#"{ "type":"response.output_text.delta", "delta":"hello", "extension":123456789012345678901234567890 }"#;
        assert!(prefix_websocket_error_payload(
            original,
            &EventTerminal::default(),
            crate::ErrorOrigin::Account,
        )
        .is_none());
    }
}

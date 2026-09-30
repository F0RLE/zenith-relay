use super::super::*;
use super::events::{handle_upstream_message, inspect_upstream_event};
use super::terminal::{
    finish_incomplete, finish_terminal, repairable_terminal_request, retryable_disconnect_request,
    retryable_terminal_request,
};
use super::{BridgeState, LiveBridge};

/// Handle one item from the upstream socket. `false` ends the bridge.
pub(super) async fn handle_upstream_item<E>(
    downstream: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    headers: &HeaderMap,
    state: &mut BridgeState,
    message: Option<Result<UpstreamMessage, E>>,
) -> bool {
    let (message, failure) = match message {
        Some(Ok(
            message @ (UpstreamMessage::Text(_)
            | UpstreamMessage::Binary(_)
            | UpstreamMessage::Ping(_)
            | UpstreamMessage::Pong(_)),
        )) => (Some(message), None),
        Some(Ok(UpstreamMessage::Close { .. })) | None => {
            (None, Some(GatewayFailure::closed(state.upstream_origin)))
        }
        Some(Err(_)) => (None, Some(GatewayFailure::transport(state.upstream_origin))),
    };
    let Some(message) = message else {
        let category = failure
            .as_ref()
            .map(|failure| failure.category)
            .unwrap_or(error_codes::UPSTREAM_WEBSOCKET_CLOSED);
        let request_id = state.request_id().map(str::to_owned);
        let stream_id = state.request_stream_id().map(str::to_owned);
        if let Some(request) = retryable_disconnect_request(runtime, state) {
            let handoff = state.retry_handoff();
            finish_incomplete(runtime, state, category);
            if handoff
                .retry(
                    &mut LiveBridge::new(downstream, upstream, runtime, key, headers, state),
                    request,
                )
                .await
            {
                return true;
            }
            return false;
        }
        let active_request = state.in_flight.is_some();
        let can_send_error = state.can_send_gateway_error();
        finish_incomplete(runtime, state, category);
        if active_request && can_send_error {
            if let Some(failure) = failure {
                send_gateway_error(
                    downstream,
                    &failure,
                    request_id.as_deref(),
                    stream_id.as_deref(),
                )
                .await;
            }
        } else if active_request {
            let _ = downstream
                .send(Message::Close(Some(CloseFrame {
                    code: close_code::ERROR,
                    reason: "upstream stream ended after output".into(),
                })))
                .await;
        }
        return false;
    };
    if let Some(request) = repairable_terminal_request(runtime, state, &message) {
        if let Some(lease) = state.lease.as_ref() {
            // Close the repair before `finish_terminal`. That
            // function settles a real failure, and a missing
            // category would otherwise stop the retry as unknown.
            lease.settle_rotation_repair(now_ms());
        }
        let handoff = state.retry_handoff();
        let mut terminal = match &message {
            UpstreamMessage::Text(text) => inspect_upstream_event(text.as_bytes(), state),
            UpstreamMessage::Binary(bytes) => inspect_upstream_event(bytes, state),
            _ => EventTerminal::default(),
        };
        if terminal.previous_response_not_found {
            terminal.error_category = Some(error_codes::RESPONSE_AFFINITY_MISS);
        }
        finish_terminal(runtime, state, terminal);
        if handoff
            .retry(
                &mut LiveBridge::new(downstream, upstream, runtime, key, headers, state),
                request,
            )
            .await
        {
            return true;
        }
        return false;
    }
    if let Some(request) = retryable_terminal_request(runtime, state, &message) {
        let handoff = state.retry_handoff();
        let terminal = match &message {
            UpstreamMessage::Text(text) => inspect_upstream_event(text.as_bytes(), state),
            UpstreamMessage::Binary(bytes) => inspect_upstream_event(bytes, state),
            _ => EventTerminal::default(),
        };
        // Do not expose a retryable pre-output terminal failure to
        // the client. `finish_terminal` retains its usual health,
        // quota, telemetry, and lease-settlement behavior first.
        finish_terminal(runtime, state, terminal);
        if handoff
            .retry(
                &mut LiveBridge::new(downstream, upstream, runtime, key, headers, state),
                request,
            )
            .await
        {
            return true;
        }
        return false;
    }
    handle_upstream_message(downstream, runtime, state, message).await
}

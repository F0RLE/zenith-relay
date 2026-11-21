use super::super::*;
use super::terminal::finish_incomplete;
use super::{BridgeState, LiveBridge};

/// Handle one item from the upstream socket. `false` ends the bridge.
pub(super) async fn handle_upstream_item<E>(
    downstream: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    runtime: &Arc<GatewayRuntime>,
    key: &AuthenticatedKey,
    headers: &HeaderMap,
    state: &mut BridgeState,
    message: Option<Result<UpstreamMessage, E>>,
) -> bool {
    if let Some(Ok(message)) = message.as_ref() {
        let scope = super::super::event_scope::EventScope::parse(message);
        let has_unscoped_owner = state.active_count() == 1
            || state
                .active_requests()
                .any(|request| request.request.stream_id.is_none());
        if scope
            .as_ref()
            .is_some_and(|scope| scope.is_connection_error(has_unscoped_owner))
        {
            finish_incomplete(runtime, state, error_codes::UPSTREAM_TERMINAL);
            let _ = forward_unowned_frame(downstream, message).await;
            return false;
        }
        if let Some(scope) = scope
            .as_ref()
            .filter(|scope| scope.is_timing_notification())
        {
            let owned = if let Some(response_id) = scope.notification_response_id() {
                state.focus_response(response_id)
            } else {
                state.active_count() == 1 && state.focus_remaining_request()
            };
            if owned && scope.matches(state.request_stream_id(), None) {
                let bytes = match message {
                    UpstreamMessage::Text(text) => text.as_bytes(),
                    UpstreamMessage::Binary(bytes) => bytes.as_ref(),
                    _ => unreachable!("only data frames have event scope"),
                };
                super::events::inspect_upstream_event(bytes, state);
            }
            // Connection measurements do not own generation, usage or setup
            // buffers, even when only one response is currently active.
            return forward_unowned_frame(downstream, message).await;
        }
        if state.active_count() > 1 {
            let size = match message {
                UpstreamMessage::Text(text) => Some(text.len()),
                UpstreamMessage::Binary(bytes) => Some(bytes.len()),
                _ => None,
            };
            let ambiguous = scope.as_ref().is_none_or(|scope| {
                scope.is_response_event()
                    && scope.stream_id().is_none()
                    && scope.response_id().is_none()
                    && !state.lane_busy(None)
            });
            if let Some(size) = size
                && ambiguous
            {
                let failure = if size > MAX_WEBSOCKET_MESSAGE_BYTES {
                    GatewayFailure::message_too_large(state.upstream_origin)
                } else {
                    GatewayFailure::classified(
                        StatusCode::BAD_GATEWAY,
                        error_codes::UPSTREAM_WEBSOCKET,
                        state.upstream_origin,
                    )
                };
                super::connection::finish_connection_failure(downstream, runtime, state, &failure)
                    .await;
                return false;
            }
            if scope.as_ref().is_some_and(|scope| {
                !scope.is_response_event()
                    && !scope.is_error()
                    && scope.stream_id().is_none()
                    && scope.response_id().is_none()
            }) {
                return forward_unowned_frame(downstream, message).await;
            }
        }
        if scope.is_some_and(|scope| !state.accept_event_scope(&scope)) {
            return true;
        }
    }
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
        let failure = failure.unwrap_or_else(|| GatewayFailure::closed(state.upstream_origin));
        super::connection::finish_connection_failure(downstream, runtime, state, &failure).await;
        return false;
    };
    super::bootstrap::handle_message(
        &mut LiveBridge::new(downstream, upstream, runtime, key, headers, state),
        message,
    )
    .await
}

async fn forward_unowned_frame(downstream: &mut WebSocket, message: &UpstreamMessage) -> bool {
    let forwarded = match message {
        UpstreamMessage::Text(text) => Message::Text(text.clone().into()),
        UpstreamMessage::Binary(bytes) => Message::Binary(bytes.clone()),
        _ => unreachable!("only data frames can be unowned events"),
    };
    send_message(downstream, forwarded).await.is_ok()
}

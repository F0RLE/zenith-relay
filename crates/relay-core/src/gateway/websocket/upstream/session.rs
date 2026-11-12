use super::super::*;
use super::connect::connect_upstream;
use super::{ConnectRecovery, Connected, ConnectedTransport};

/// Keeps a persistent pre-output retry cancellable by the client. Candidate
/// recovery may wait indefinitely, but a closed client WebSocket must release
/// the request and its lease immediately.
#[allow(clippy::too_many_arguments)]
pub(in crate::gateway::websocket) async fn connect_upstream_while_client_connected(
    downstream: &mut WebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    client_headers: &HeaderMap,
    request: ClientRequest,
    allow_previous_response_reset: bool,
    attempt_offset: u16,
    recovery: Option<ConnectRecovery>,
) -> Result<Connected, GatewayFailure> {
    match connect_transport_while_client_connected(
        downstream,
        runtime,
        key,
        client_headers,
        request,
        allow_previous_response_reset,
        attempt_offset,
        recovery,
    )
    .await?
    {
        ConnectedTransport::Websocket(connected) => Ok(*connected),
        // Existing native-socket reconnects retain their transport boundary.
        // Dropping this unstarted handoff releases its capacity normally.
        ConnectedTransport::Http(fallback) => Err(
            if fallback.reason == crate::usage::TransportFallbackReason::WebsocketSlowHandshake {
                GatewayFailure::websocket_slow_handshake(fallback.origin)
            } else {
                GatewayFailure::websocket_http_fallback(fallback.origin)
            },
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub(in crate::gateway::websocket) async fn connect_transport_while_client_connected(
    downstream: &mut WebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    client_headers: &HeaderMap,
    request: ClientRequest,
    allow_previous_response_reset: bool,
    attempt_offset: u16,
    recovery: Option<ConnectRecovery>,
) -> Result<ConnectedTransport, GatewayFailure> {
    let stream_id = request.stream_id.clone();
    await_while_client_connected(
        downstream,
        // Reconnects run inside the live bridge; keep the retry future off its stack.
        Box::pin(connect_upstream(
            runtime,
            key,
            client_headers,
            request,
            allow_previous_response_reset,
            attempt_offset,
            recovery,
        )),
        stream_id.as_deref(),
    )
    .await?
}

pub(in crate::gateway::websocket) async fn await_while_client_connected<F: std::future::Future>(
    downstream: &mut WebSocket,
    future: F,
    stream_id: Option<&str>,
) -> Result<F::Output, GatewayFailure> {
    tokio::pin!(future);
    let mut heartbeat = interval_at(
        TokioInstant::now() + WEBSOCKET_HEARTBEAT_INTERVAL,
        WEBSOCKET_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            connection_result = &mut future => return Ok(connection_result),
            _ = heartbeat.tick() => {
                send_message(downstream, Message::Ping(Default::default()))
                    .await
                    .map_err(|_| GatewayFailure::client_closed())?;
            }
            message = downstream.recv_live() => {
                match message {
                    Some(Ok(Message::Ping(ping_payload))) => {
                        send_message(downstream, Message::Pong(ping_payload))
                            .await
                            .map_err(|_| GatewayFailure::client_closed())?;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                        return Err(GatewayFailure::client_closed());
                    }
                    Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                        if downstream.defer_request(&message).await? {
                            continue;
                        }
                        let payload: &[u8] = match &message {
                            Message::Text(text) => text.as_bytes(),
                            Message::Binary(bytes) => bytes,
                            _ => unreachable!("matched a data frame"),
                        };
                        let Some(reply) = unavailable_interrupt(payload, stream_id) else {
                            let failure = GatewayFailure::invalid_request(
                                "unsupported control while a response is in progress",
                            );
                            super::super::failure::send_request_error(
                                downstream,
                                &failure,
                                None,
                                ClientRequest::error_stream_id(payload).as_deref(),
                            )
                            .await
                            .map_err(|_| GatewayFailure::client_closed())?;
                            continue;
                        };
                        send_message(downstream, reply)
                            .await
                            .map_err(|_| GatewayFailure::client_closed())?;
                    }
                }
            }
        }
    }
}

fn unavailable_interrupt(payload: &[u8], stream_id: Option<&str>) -> Option<Message> {
    if payload.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
        return None;
    }
    let event: Value = serde_json::from_slice(payload).ok()?;
    if event.get("type")?.as_str()? != "response.interrupt"
        || event.get("mode")?.as_str()? != "discard_partial_items"
        || event.get("stream_id").is_some_and(|requested| {
            requested
                .as_str()
                .is_none_or(|requested| Some(requested) != stream_id)
        })
    {
        return None;
    }
    let response_id = event
        .get("response_id")?
        .as_str()
        .filter(|id| !id.is_empty())?;
    let mut reply = json!({
        "type": "response.interrupt.failed",
        "response_id": response_id,
    });
    if let Some(stream_id) = stream_id {
        reply["stream_id"] = json!(stream_id);
    }
    Some(Message::Text(reply.to_string().into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_interrupt_validates_control_and_lane() {
        for event in [
            json!({"type":"response.interrupt","mode":"unknown","response_id":"response"}),
            json!({"type":"response.cancel","mode":"discard_partial_items","response_id":"response"}),
            json!({"type":"response.interrupt","mode":"discard_partial_items","response_id":""}),
            json!({"type":"response.interrupt","mode":"discard_partial_items","response_id":1}),
            json!({"type":"response.interrupt","mode":"discard_partial_items"}),
            json!({"type":"response.interrupt","mode":"discard_partial_items","response_id":"response","stream_id":"other"}),
            json!({"type":"response.interrupt","mode":"discard_partial_items","response_id":"response","stream_id":1}),
            json!({"type":"response.interrupt","mode":"discard_partial_items","response_id":"response","stream_id":null}),
        ] {
            for stream_id in [None, Some("main")] {
                assert!(
                    unavailable_interrupt(&serde_json::to_vec(&event).unwrap(), stream_id)
                        .is_none()
                );
            }
        }
        assert!(unavailable_interrupt(b"not json", None).is_none());
    }
}

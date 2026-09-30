use super::super::*;
use super::connect::connect_upstream;
use super::Connected;

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
) -> Result<Connected, GatewayFailure> {
    await_while_client_connected(
        downstream,
        connect_upstream(
            runtime,
            key,
            client_headers,
            request,
            allow_previous_response_reset,
            attempt_offset,
        ),
    )
    .await?
}

pub(in crate::gateway::websocket) async fn await_while_client_connected<F: std::future::Future>(
    downstream: &mut WebSocket,
    future: F,
) -> Result<F::Output, GatewayFailure> {
    tokio::pin!(future);
    let mut heartbeat = interval_at(
        TokioInstant::now() + WEBSOCKET_HEARTBEAT_INTERVAL,
        WEBSOCKET_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            result = &mut future => return Ok(result),
            _ = heartbeat.tick() => {
                downstream
                    .send(Message::Ping(Default::default()))
                    .await
                    .map_err(|_| GatewayFailure::client_closed())?;
            }
            message = downstream.recv() => {
                match message {
                    Some(Ok(Message::Ping(payload))) => {
                        downstream
                            .send(Message::Pong(payload))
                            .await
                            .map_err(|_| GatewayFailure::client_closed())?;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                        return Err(GatewayFailure::client_closed());
                    }
                    Some(Ok(Message::Text(_))) | Some(Ok(Message::Binary(_))) => {
                        return Err(GatewayFailure::invalid_request(
                            "a response is already in progress",
                        ));
                    }
                }
            }
        }
    }
}

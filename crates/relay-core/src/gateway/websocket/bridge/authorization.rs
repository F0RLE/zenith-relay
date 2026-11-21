use super::super::upstream::{ConnectRecovery, ConnectTrace, record_connect_failure};
use super::super::*;
use super::{BridgeState, LiveBridge, RetryHandoff};

pub(super) struct PendingAdmission {
    pub(super) request: ClientRequest,
    pub(super) route: ExecutorRoute,
    pub(super) lease: CandidateLease,
    pub(super) response_affinity_hit: bool,
    pub(super) started: Instant,
}

pub(super) struct ReadyAdmission {
    pub(super) pending: PendingAdmission,
    pub(super) authorization: crate::runtime::PreparedAuthorization,
}

pub(super) enum Preparation {
    Ready(Box<ReadyAdmission>),
    Rejected,
    Closed,
}

pub(super) fn defer(state: &mut BridgeState, request: ClientRequest) {
    state.pending_retries.push_back((
        RetryHandoff {
            request_id: Some(request.request_id.clone()),
            attempt_offset: 0,
            recovery: ConnectRecovery::new(true),
        },
        request,
    ));
}

pub(super) fn route_matches(
    state: &BridgeState,
    route: &ExecutorRoute,
    request: &ClientRequest,
) -> bool {
    route.candidate_id == state.upstream_candidate_id
        && route.binding_key() == state.upstream_route.binding_key()
        && (route.account_id.is_some() && request.responses_lite) == state.upstream_responses_lite
        && route.source_model == state.upstream_route.source_model
        && route.upstream_url == state.upstream_route.upstream_url
        && route.upstream_headers == state.upstream_route.upstream_headers
}

/// Authorization renewal owns durable account state, not the client socket.
/// A disconnected client drops its reservation while a started rotation can
/// finish persistence. The background task never owns a generation lease.
pub(super) async fn prepare(bridge: &mut LiveBridge<'_>, pending: PendingAdmission) -> Preparation {
    match prepare_inner(bridge, pending).await {
        Ok(preparation) => preparation,
        Err(failure) => {
            super::connection::finish_connection_failure(
                bridge.downstream,
                bridge.runtime,
                bridge.bridge_state,
                &failure,
            )
            .await;
            Preparation::Closed
        }
    }
}

async fn prepare_inner(
    bridge: &mut LiveBridge<'_>,
    pending: PendingAdmission,
) -> Result<Preparation, GatewayFailure> {
    debug_assert!(bridge.bridge_state.pending_admission.is_none());
    let runtime = bridge.runtime.clone();
    let route = pending.route.clone();
    let origin = route_error_origin(&route);
    let mut task =
        tokio::spawn(async move { runtime.prepare_route_authorization(&route, now_ms()).await });
    bridge.bridge_state.pending_admission = Some(pending);
    let mut heartbeat = interval_at(
        TokioInstant::now() + WEBSOCKET_HEARTBEAT_INTERVAL,
        WEBSOCKET_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let prepared = loop {
        tokio::select! {
            result = &mut task => break match result {
                Ok(result) => result.map_err(|error| GatewayFailure::prepare(error, origin)),
                Err(_) => Err(GatewayFailure::prepare(ExecutorPrepareError::Transient, ErrorOrigin::Relay)),
            },
            message = bridge.upstream.next() => {
                if !super::upstream_item::handle_upstream_item(
                    bridge.downstream, bridge.upstream, bridge.runtime, bridge.key,
                    bridge.headers, bridge.bridge_state, message,
                ).await {
                    super::terminal::finish_incomplete(bridge.runtime, bridge.bridge_state, error_codes::UPSTREAM_WEBSOCKET);
                    return Ok(Preparation::Closed);
                }
            }
            message = bridge.downstream.recv_live() => {
                let Some(Ok(message)) = message else {
                    super::terminal::finish_incomplete(bridge.runtime, bridge.bridge_state, error_codes::CLIENT_CANCELLED);
                    return Ok(Preparation::Closed);
                };
                match message {
                    message @ (Message::Text(_) | Message::Binary(_)) => {
                        if !bridge.downstream.defer_request(&message).await? {
                            let bytes = match &message { Message::Text(text) => text.as_bytes(), Message::Binary(bytes) => bytes.as_ref(), _ => unreachable!() };
                            super::super::failure::send_request_error(bridge.downstream,
                                &GatewayFailure::invalid_request("only response.create messages are supported"),
                                None, ClientRequest::error_stream_id(bytes).as_deref()).await.map_err(|_| GatewayFailure::client_closed())?;
                        }
                    }
                    Message::Ping(bytes) => {
                        send_message(bridge.upstream, UpstreamMessage::Ping(bytes)).await.map_err(|_| GatewayFailure::transport(bridge.bridge_state.upstream_origin))?;
                    }
                    Message::Pong(bytes) => {
                        send_message(bridge.upstream, UpstreamMessage::Pong(bytes)).await.map_err(|_| GatewayFailure::transport(bridge.bridge_state.upstream_origin))?;
                    }
                    Message::Close(frame) => {
                        super::terminal::finish_incomplete(bridge.runtime, bridge.bridge_state, error_codes::CLIENT_CANCELLED);
                        let _ = super::super::io::send_close_message(bridge.downstream, Message::Close(frame)).await;
                        return Ok(Preparation::Closed);
                    }
                }
            }
            _ = heartbeat.tick() => {
                if send_message(bridge.downstream, Message::Ping(Default::default())).await.is_err() {
                    super::terminal::finish_incomplete(bridge.runtime, bridge.bridge_state, error_codes::CLIENT_CANCELLED);
                    return Ok(Preparation::Closed);
                }
                send_message(bridge.upstream, UpstreamMessage::Ping(Default::default())).await.map_err(|_| GatewayFailure::transport(bridge.bridge_state.upstream_origin))?;
            }
        }
    };
    bridge.bridge_state.park_current_request();
    let Some(pending) = bridge.bridge_state.pending_admission.take() else {
        return Ok(Preparation::Closed);
    };
    match prepared {
        Ok(authorization) => Ok(Preparation::Ready(Box::new(ReadyAdmission {
            pending,
            authorization,
        }))),
        Err(failure) => {
            record_connect_failure(
                &ConnectTrace {
                    event: None,
                    runtime: bridge.runtime,
                    lease: &pending.lease,
                    key: bridge.key,
                    route: &pending.route,
                    request: &pending.request,
                    attempt: 0,
                    started: pending.started,
                },
                &failure,
                None,
            );
            let id = pending.request.request_id.clone();
            let lane = pending.request.stream_id.clone();
            drop(pending);
            if super::super::failure::send_request_error(
                bridge.downstream,
                &failure,
                Some(&id),
                lane.as_deref(),
            )
            .await
            .is_err()
            {
                super::terminal::finish_incomplete(
                    bridge.runtime,
                    bridge.bridge_state,
                    error_codes::CLIENT_CANCELLED,
                );
                return Ok(Preparation::Closed);
            }
            Ok(Preparation::Rejected)
        }
    }
}

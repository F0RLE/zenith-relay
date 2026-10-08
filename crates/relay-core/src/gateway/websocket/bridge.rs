mod connection;
mod downstream;
mod events;
mod terminal;
mod upstream_item;

use super::upstream::Connected;
use super::*;
use connection::retry_upstream_connection;
use downstream::handle_downstream_message;
use events::handle_upstream_message;
use terminal::finish_incomplete;
use upstream_item::handle_upstream_item;

pub(in crate::gateway::websocket) use connection::send_request;
pub(in crate::gateway::websocket) use events::semantic_output_payload;

pub(super) struct InFlight {
    request: ClientRequest,
    route: ExecutorRoute,
    event: UsageEvent,
    started: Instant,
    response_id: Option<String>,
    prompt_affinity_key: Option<String>,
    client_visible_output: bool,
    legacy_call_id_repair_attempted: bool,
    native_replay_capture: NativeReplayCapture,
}

pub(super) struct BridgeState {
    credential_fingerprint: [u8; 32],
    authorization_incarnation: AuthorizationIncarnation,
    local_key_id: String,
    lease: Option<CandidateLease>,
    in_flight: Option<InFlight>,
    stream_id: Option<String>,
    upstream_candidate_id: String,
    upstream_origin: ErrorOrigin,
    last_response_id: Option<String>,
    transient_response_affinity_key: Option<String>,
}

impl BridgeState {
    fn can_send_gateway_error(&self) -> bool {
        self.in_flight
            .as_ref()
            .is_none_or(|in_flight| !in_flight.client_visible_output)
    }

    fn request_id(&self) -> Option<&str> {
        self.in_flight
            .as_ref()
            .map(|in_flight| in_flight.event.request_id.as_str())
    }

    fn request_stream_id(&self) -> Option<&str> {
        self.in_flight
            .as_ref()
            .and_then(|in_flight| in_flight.request.stream_id.as_deref())
    }

    fn retry_handoff(&self) -> RetryHandoff {
        RetryHandoff {
            request_id: self.request_id().map(str::to_owned),
            attempt_offset: self
                .in_flight
                .as_ref()
                .map(|in_flight| in_flight.event.attempt)
                .unwrap_or_default(),
        }
    }
}

struct RetryHandoff {
    request_id: Option<String>,
    attempt_offset: u16,
}

struct LiveBridge<'a> {
    downstream: &'a mut WebSocket,
    upstream: &'a mut UpstreamWebSocket,
    runtime: &'a GatewayRuntime,
    key: &'a AuthenticatedKey,
    headers: &'a HeaderMap,
    bridge_state: &'a mut BridgeState,
}

impl<'a> LiveBridge<'a> {
    fn new(
        downstream: &'a mut WebSocket,
        upstream: &'a mut UpstreamWebSocket,
        runtime: &'a GatewayRuntime,
        key: &'a AuthenticatedKey,
        headers: &'a HeaderMap,
        bridge_state: &'a mut BridgeState,
    ) -> Self {
        Self {
            downstream,
            upstream,
            runtime,
            key,
            headers,
            bridge_state,
        }
    }
}

impl RetryHandoff {
    async fn retry(self, bridge: &mut LiveBridge<'_>, client_request: ClientRequest) -> bool {
        retry_upstream_connection(
            bridge,
            client_request,
            self.attempt_offset,
            self.request_id.as_deref(),
        )
        .await
    }
}

pub(super) async fn bridge(
    mut downstream: WebSocket,
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    headers: HeaderMap,
    connected: Connected,
) {
    let mut upstream = connected.upstream;
    let initial_event = usage_event(
        UsageAttempt {
            request_id: &connected.request.request_id,
            attempt: connected.attempt,
            local_key_id: &key.id,
            route: &connected.route,
            reasoning_effort: Some(&connected.request.reasoning_effort_for(&connected.route)),
            requested_model: &connected.request.requested_model,
            tool_use: connected.request.tool_use_for(&connected.route),
        },
        true,
        StatusCode::OK.as_u16(),
        None,
        0,
    );
    let upstream_candidate_id = connected.route.candidate_id.clone();
    let upstream_origin = route_error_origin(&connected.route);
    let prompt_affinity_key = connected.request.prompt_affinity_key.clone();
    let mut bridge_state = BridgeState {
        credential_fingerprint: connected.credential_fingerprint,
        authorization_incarnation: connected.authorization_incarnation,
        local_key_id: key.id.clone(),
        lease: Some(connected.lease),
        in_flight: Some(InFlight {
            request: connected.request.clone(),
            route: connected.route,
            event: initial_event,
            started: connected.started,
            response_id: None,
            prompt_affinity_key,
            client_visible_output: false,
            legacy_call_id_repair_attempted: false,
            native_replay_capture: NativeReplayCapture::default(),
        }),
        stream_id: connected.request.stream_id.clone(),
        upstream_candidate_id,
        upstream_origin,
        last_response_id: None,
        transient_response_affinity_key: None,
    };
    for message in connected.initial_messages {
        if !handle_upstream_message(&mut downstream, &runtime, &mut bridge_state, message).await {
            return;
        }
    }
    let mut last_activity = TokioInstant::now();
    let mut heartbeat = interval_at(
        TokioInstant::now() + WEBSOCKET_HEARTBEAT_INTERVAL,
        WEBSOCKET_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        let idle_deadline = last_activity + WEBSOCKET_IDLE_TIMEOUT;
        tokio::select! {
            // Reap an unused connection, never a provider's active generation.
            _ = sleep_until(idle_deadline), if bridge_state.in_flight.is_none() => {
                let _ = downstream.send(Message::Close(Some(CloseFrame {
                    code: close_code::AWAY,
                    reason: "idle timeout".into(),
                }))).await;
                break;
            }
            _ = heartbeat.tick() => {
                if downstream.send(Message::Ping(Default::default())).await.is_err() {
                    finish_incomplete(&runtime, &mut bridge_state, error_codes::CLIENT_CANCELLED);
                    break;
                }
                if upstream.send(UpstreamMessage::Ping(Default::default())).await.is_err() {
                    let active_request = bridge_state.in_flight.is_some()
                        && bridge_state.can_send_gateway_error();
                    let request_id = bridge_state.request_id().map(str::to_owned);
                    let stream_id = bridge_state.request_stream_id().map(str::to_owned);
                    finish_incomplete(
                        &runtime,
                        &mut bridge_state,
                        error_codes::UPSTREAM_WEBSOCKET,
                    );
                    if active_request {
                        send_gateway_error(
                            &mut downstream,
                            &GatewayFailure::transport(bridge_state.upstream_origin),
                            request_id.as_deref(),
                            stream_id.as_deref(),
                        )
                        .await;
                    }
                    break;
                }
            }
            message = downstream.recv() => {
                last_activity = TokioInstant::now();
                let Some(message) = message else {
                    finish_incomplete(&runtime, &mut bridge_state, error_codes::CLIENT_CANCELLED);
                    break;
                };
                let Ok(message) = message else {
                    finish_incomplete(&runtime, &mut bridge_state, "client_websocket");
                    break;
                };
                match handle_downstream_message(
                    &mut downstream,
                    &mut upstream,
                    &runtime,
                    &key,
                    &headers,
                    &mut bridge_state,
                    message,
                ).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(failure) => {
                        let request_id = bridge_state.request_id().map(str::to_owned);
                        let stream_id = bridge_state.request_stream_id().map(str::to_owned);
                        let can_send_error = bridge_state.can_send_gateway_error();
                        finish_incomplete(&runtime, &mut bridge_state, failure.category);
                        if can_send_error {
                            send_gateway_error(
                                &mut downstream,
                                &failure,
                                request_id.as_deref(),
                                stream_id.as_deref(),
                            )
                            .await;
                        }
                        break;
                    }
                }
            }
            message = upstream.next() => {
                last_activity = TokioInstant::now();
                if !handle_upstream_item(
                    &mut downstream,
                    &mut upstream,
                    &runtime,
                    &key,
                    &headers,
                    &mut bridge_state,
                    message,
                )
                .await
                {
                    break;
                }
            }
        }
    }
}

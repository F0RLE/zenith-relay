mod active;
mod authorization;
mod bootstrap;
mod connection;
mod downstream;
mod events;
mod terminal;
mod upstream_item;

use super::upstream::Connected;
use super::*;
use connection::retry_upstream_connection;
use downstream::handle_downstream_message;
use terminal::finish_incomplete;
use upstream_item::handle_upstream_item;

pub(in crate::gateway::websocket) use connection::send_request;
pub(in crate::gateway::websocket) use events::{semantic_output_payload, semantic_output_value};

pub(super) struct InFlight {
    lease: CandidateLease,
    request: ClientRequest,
    route: ExecutorRoute,
    event: UsageEvent,
    usage_snapshot: Option<Value>,
    started: Instant,
    response_id: Option<String>,
    prompt_affinity_key: Option<String>,
    client_visible_output: bool,
    prelude: Option<bootstrap::PendingResponse>,
    tool_call_capture: crate::gateway::request::ToolCallCapture,
    native_replay_capture: NativeReplayCapture,
}

pub(super) struct BridgeState {
    connection: ConnectionState,
    http_handoff: Option<connection::HttpHandoff>,
    pending_admission: Option<authorization::PendingAdmission>,
    credential_fingerprint: [u8; 32],
    authorization_incarnation: AuthorizationIncarnation,
    local_key_id: String,
    in_flight: Option<InFlight>,
    other_requests: std::collections::BTreeMap<Option<String>, InFlight>,
    pending_retries: std::collections::VecDeque<(RetryHandoff, ClientRequest)>,
    upstream_candidate_id: String,
    upstream_responses_lite: bool,
    upstream_search_declared: Option<bool>,
    upstream_route: ExecutorRoute,
    upstream_origin: ErrorOrigin,
    responses: super::connection_responses::ConnectionResponses,
}

enum ConnectionState {
    Native,
    Http(Option<String>),
}

impl BridgeState {
    fn accept_event_scope(&mut self, scope: &super::event_scope::EventScope<'_>) -> bool {
        if self.active_count() == 1 {
            self.focus_remaining_request();
        }
        if let Some(lane) = scope.stream_id() {
            if !self.focus_lane(Some(lane)) {
                return false;
            }
        } else if let Some(response_id) = scope.response_id() {
            if !self.focus_response(response_id)
                && !self.focus_lane(None)
                && self.active_count() > 1
            {
                return false;
            }
        } else if self.active_count() > 1 && !self.focus_lane(None) {
            return false;
        }
        if scope.response_id().is_some_and(|id| {
            self.responses.contains(id)
                && !self
                    .responses
                    .contains_on_lane(self.request_stream_id(), id)
                && !self
                    .active_requests()
                    .any(|request| request.response_id.as_deref() == Some(id))
        }) {
            return false;
        }
        if scope.response_id().is_some_and(|response_id| {
            self.other_requests
                .values()
                .any(|request| request.response_id.as_deref() == Some(response_id))
        }) {
            return false;
        }
        if !scope.matches(
            self.request_stream_id(),
            self.in_flight
                .as_ref()
                .and_then(|request| request.response_id.as_deref()),
        ) {
            return false;
        }
        let Some(in_flight) = self.in_flight.as_mut() else {
            return scope.response_id().is_none();
        };
        if in_flight.response_id.is_none() {
            in_flight.response_id = scope.response_id().map(str::to_owned);
        }
        true
    }

    fn websocket_authorization(&self) -> super::request::WebsocketAuthorization {
        super::request::WebsocketAuthorization {
            candidate_id: self.upstream_candidate_id.clone(),
            credential_fingerprint: self.credential_fingerprint,
            incarnation: self.authorization_incarnation.clone(),
        }
    }

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
}

struct RetryHandoff {
    request_id: Option<String>,
    attempt_offset: u16,
    recovery: super::upstream::ConnectRecovery,
}

struct LiveBridge<'a> {
    downstream: &'a mut WebSocket,
    upstream: &'a mut UpstreamWebSocket,
    runtime: &'a Arc<GatewayRuntime>,
    key: &'a AuthenticatedKey,
    headers: &'a HeaderMap,
    bridge_state: &'a mut BridgeState,
}

impl<'a> LiveBridge<'a> {
    fn new(
        downstream: &'a mut WebSocket,
        upstream: &'a mut UpstreamWebSocket,
        runtime: &'a Arc<GatewayRuntime>,
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
            self.recovery,
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
            reasoning_effort: Some(&connected.reasoning_effort),
            requested_model: &connected.request.requested_model,
            tool_use: connected.tool_use,
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
        connection: ConnectionState::Native,
        http_handoff: None,
        pending_admission: None,
        credential_fingerprint: connected.credential_fingerprint,
        authorization_incarnation: connected.authorization_incarnation,
        local_key_id: key.id.clone(),
        in_flight: Some(InFlight {
            lease: connected.lease,
            request: connected.request.clone(),
            route: connected.route.clone(),
            event: initial_event,
            usage_snapshot: None,
            started: connected.started,
            response_id: None,
            prompt_affinity_key,
            client_visible_output: false,
            prelude: Some(bootstrap::PendingResponse::new(
                connected.recovery,
                connected.response_affinity_hit,
                true,
            )),
            tool_call_capture: Default::default(),
            native_replay_capture: NativeReplayCapture::default(),
        }),
        other_requests: Default::default(),
        pending_retries: Default::default(),
        upstream_candidate_id,
        upstream_responses_lite: connected.route.account_id.is_some()
            && connected.request.responses_lite,
        upstream_search_declared: connected.native_search_declared,
        upstream_route: connected.route.clone(),
        upstream_origin,
        responses: super::connection_responses::ConnectionResponses::new(
            connected.request.stream_id.clone(),
        ),
    };
    let mut last_activity = TokioInstant::now();
    let mut heartbeat = interval_at(
        TokioInstant::now() + WEBSOCKET_HEARTBEAT_INTERVAL,
        WEBSOCKET_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        if bridge_state.active_count() == 0
            && let Some((handoff, request)) = bridge_state.pending_retries.pop_front()
            && !handoff
                .retry(
                    &mut LiveBridge::new(
                        &mut downstream,
                        &mut upstream,
                        &runtime,
                        &key,
                        &headers,
                        &mut bridge_state,
                    ),
                    request,
                )
                .await
        {
            break;
        }
        if let Some(handoff) = bridge_state.http_handoff.take() {
            terminal::clear_connection_responses(&runtime, &mut bridge_state);
            bridge_state.connection = ConnectionState::Http(handoff.request.stream_id.clone());
            if bridge_state.pending_retries.is_empty() {
                drop(upstream);
                super::fallback::bridge_http_fallback(downstream, runtime, key, headers, handoff)
                    .await;
                return;
            }
            if !super::fallback::serve_http_fallback_once(
                &mut downstream,
                runtime.clone(),
                &headers,
                handoff,
            )
            .await
            {
                break;
            }
            continue;
        }
        if let ConnectionState::Http(lane) = &bridge_state.connection {
            if !bridge_state.pending_retries.is_empty() {
                continue;
            }
            let lane = lane.clone();
            drop(upstream);
            super::fallback::resume_http_fallback(downstream, runtime, key, headers, lane).await;
            return;
        }
        let idle_deadline = last_activity + WEBSOCKET_IDLE_TIMEOUT;
        tokio::select! {
            // Reap an unused connection, never a provider's active generation.
            _ = sleep_until(idle_deadline), if bridge_state.active_count() == 0 => {
                let _ = send_message(&mut downstream, Message::Close(Some(CloseFrame {
                    code: close_code::AWAY,
                    reason: "idle timeout".into(),
                }))).await;
                break;
            }
            _ = heartbeat.tick() => {
                if send_message(&mut downstream, Message::Ping(Default::default())).await.is_err() {
                    finish_incomplete(&runtime, &mut bridge_state, error_codes::CLIENT_CANCELLED);
                    break;
                }
                if send_message(&mut upstream, UpstreamMessage::Ping(Default::default())).await.is_err() {
                    let failure = GatewayFailure::transport(bridge_state.upstream_origin);
                    connection::finish_connection_failure(&mut downstream, &runtime, &mut bridge_state, &failure).await;
                    break;
                }
            }
            message = downstream.recv_native(
                bridge_state.active_count() == 0,
                |lane| bridge_state.can_start_lane(lane),
            ) => {
                last_activity = TokioInstant::now();
                let Some((message, queued)) = message else {
                    finish_incomplete(&runtime, &mut bridge_state, error_codes::CLIENT_CANCELLED);
                    break;
                };
                let Ok(message) = message else {
                    finish_incomplete(&runtime, &mut bridge_state, "client_websocket");
                    break;
                };
                let lane = match &message {
                    Message::Text(text) => ClientRequest::error_stream_id(text.as_bytes()),
                    Message::Binary(bytes) => ClientRequest::error_stream_id(bytes),
                    _ => None,
                };
                if !queued && (!bridge_state.can_start_lane(lane.as_deref())
                    || (bridge_state.active_count() > 0 && downstream.waits_for_idle())
                    || downstream.has_pending_lane(lane.as_deref()))
                {
                    match downstream.defer_request(&message).await {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(failure) => {
                            finish_incomplete(&runtime, &mut bridge_state, failure.category);
                            break;
                        }
                    }
                }
                if matches!(message, Message::Text(_) | Message::Binary(_)) {
                    bridge_state.park_current_request();
                }
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
                        if bridge_state.in_flight.is_none()
                            && super::failure::send_request_error(
                                &mut downstream, &failure, None, lane.as_deref(),
                            ).await.is_ok() {
                                continue;
                        }
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
                last_activity = TokioInstant::now();
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
    finish_incomplete(&runtime, &mut bridge_state, error_codes::CLIENT_CANCELLED);
}

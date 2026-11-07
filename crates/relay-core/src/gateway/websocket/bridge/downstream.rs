use super::super::*;
use super::connection::{install_connected, send_request};
use super::terminal::{clear_connection_responses, finish_incomplete};
use super::{BridgeState, InFlight, LiveBridge, authorization};

pub(super) async fn handle_downstream_message(
    downstream: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    runtime: &Arc<GatewayRuntime>,
    key: &AuthenticatedKey,
    headers: &HeaderMap,
    state: &mut BridgeState,
    message: Message,
) -> Result<bool, GatewayFailure> {
    match message {
        Message::Text(text) => {
            return start_next_request(
                downstream,
                upstream,
                runtime,
                key,
                headers,
                state,
                text.as_bytes(),
            )
            .await;
        }
        Message::Binary(bytes) => {
            return start_next_request(downstream, upstream, runtime, key, headers, state, &bytes)
                .await;
        }
        Message::Ping(ping_payload) => {
            send_message(upstream, UpstreamMessage::Ping(ping_payload))
                .await
                .map_err(|_| GatewayFailure::transport(state.upstream_origin))?;
        }
        Message::Pong(pong_payload) => {
            send_message(upstream, UpstreamMessage::Pong(pong_payload))
                .await
                .map_err(|_| GatewayFailure::transport(state.upstream_origin))?;
        }
        Message::Close(frame) => {
            // Commit observed usage and release capacity before any best-effort
            // close handshake with a peer that may have stopped reading.
            finish_incomplete(runtime, state, error_codes::CLIENT_CANCELLED);
            let (code, reason) = frame
                .map(|frame| {
                    (
                        UpstreamCloseCode::from(frame.code),
                        frame.reason.to_string(),
                    )
                })
                .unwrap_or((UpstreamCloseCode::Normal, String::new()));
            let _ = super::super::io::send_close_message(
                upstream,
                UpstreamMessage::Close { code, reason },
            )
            .await;
            let _ = super::super::io::send_close_message(downstream, Message::Close(None)).await;
            return Ok(false);
        }
    }
    Ok(true)
}

async fn start_next_request(
    downstream: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    runtime: &Arc<GatewayRuntime>,
    key: &AuthenticatedKey,
    headers: &HeaderMap,
    state: &mut BridgeState,
    request_bytes: &[u8],
) -> Result<bool, GatewayFailure> {
    if state.in_flight.is_some() {
        let failure = GatewayFailure::invalid_request("a response is already in progress");
        super::super::failure::send_request_error(
            downstream,
            &failure,
            None,
            ClientRequest::error_stream_id(request_bytes).as_deref(),
        )
        .await
        .map_err(|_| GatewayFailure::client_closed())?;
        return Ok(true);
    }
    let prepared =
        ClientRequest::prepare(runtime, key, headers, request_bytes, Some(&state.responses)).await;
    let mut client_request = match prepared {
        Ok(request) => request,
        Err(failure) => {
            super::super::failure::send_request_error(
                downstream,
                &failure,
                None,
                ClientRequest::error_stream_id(request_bytes).as_deref(),
            )
            .await
            .map_err(|_| GatewayFailure::client_closed())?;
            return Ok(true);
        }
    };
    client_request.websocket_authorization = Some(state.websocket_authorization());
    if let Err(failure) = state
        .responses
        .register(client_request.stream_id.as_deref())
    {
        super::super::failure::send_request_error(
            downstream,
            &failure,
            Some(&client_request.request_id),
            client_request.stream_id.as_deref(),
        )
        .await
        .map_err(|_| GatewayFailure::client_closed())?;
        return Ok(true);
    }
    let same_response_id = client_request
        .previous_response_id()
        .is_some_and(|response_id| state.responses.contains(response_id));
    let concurrent = !state.other_requests.is_empty();
    let response_affinity_key = client_request.response_affinity_key.clone();
    let model_switch = same_response_id
        && !client_request.has_unpaired_tool_output()
        && response_affinity_key.as_deref().and_then(|affinity_key| {
            runtime.response_affinity_owner_supports_route(
                key,
                affinity_key,
                &client_request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                now_ms(),
            )
        }) == Some(false);
    let owner_should_yield = response_affinity_key
        .as_deref()
        .is_some_and(|affinity_key| {
            runtime.automatic_response_owner_should_yield(
                key,
                affinity_key,
                &client_request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                &Default::default(),
                now_ms(),
            )
        });
    if concurrent && (model_switch || owner_should_yield) {
        downstream.defer_until_idle(request_bytes).await?;
        return Ok(true);
    }
    let model_switch_reset =
        model_switch && client_request.drop_previous_response_id(runtime, &key.id);
    if model_switch_reset {
        clear_connection_responses(runtime, state);
    }
    let quota_yield = owner_should_yield && client_request.yield_response_owner(runtime, &key.id);
    if (same_response_id || concurrent) && !model_switch_reset && !quota_yield {
        let (tried, mut admitted) = client_request.compatible_routes(runtime, key)?;
        let selected = runtime
            .select_and_reserve_with_budget(
                key,
                &client_request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                &tried,
                (
                    client_request.response_affinity_key.as_deref(),
                    client_request.prompt_affinity_key.as_deref(),
                ),
                now_ms(),
                &client_request.budget,
            )
            .await;
        let Some((selected, lease)) = selected else {
            if concurrent {
                downstream.defer_until_idle(request_bytes).await?;
                return Ok(true);
            }
            // The owner can become temporarily ineligible while this client
            // WebSocket stays open because another chat consumed its quota.
            // Reuse would fail before reaching the owner, so reconnect through
            // the normal bounded replay path. That path materializes the local
            // Responses history and removes the opaque owner reference before
            // choosing a compatible account or API source.
            if client_request.has_previous_response_id() && client_request.requires_affinity_owner {
                let connected = connect_upstream_while_client_connected(
                    downstream,
                    runtime,
                    key,
                    headers,
                    client_request,
                    false,
                    0,
                    None,
                )
                .await?;
                return Ok(install_connected(upstream, runtime, key, state, connected));
            }
            if let Some(retry_at_ms) = runtime.earliest_retry_at(
                key,
                &client_request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                &tried,
                client_request.response_affinity_key.as_deref(),
                now_ms(),
                crate::scheduler::rotation::RotationOperation::Text,
            ) {
                return Err(GatewayFailure::cooldown(retry_at_ms));
            }
            return Err(GatewayFailure::unavailable());
        };
        if selected.candidate_id != state.upstream_candidate_id {
            if concurrent {
                drop(lease);
                downstream.defer_until_idle(request_bytes).await?;
                return Ok(true);
            }
            return Err(GatewayFailure::unavailable());
        }
        let mut route = admitted
            .remove(&selected.candidate_id)
            .ok_or_else(GatewayFailure::unavailable)?;
        runtime.set_route_streaming(&mut route, false);
        if !authorization::route_matches(state, &route, &client_request) {
            drop(lease);
            if concurrent {
                downstream.defer_until_idle(request_bytes).await?;
                return Ok(true);
            }
            let connected = connect_upstream_while_client_connected(
                downstream,
                runtime,
                key,
                headers,
                client_request,
                false,
                0,
                None,
            )
            .await?;
            return Ok(install_connected(upstream, runtime, key, state, connected));
        }
        route.client_transport = crate::UsageTransport::Websocket;
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.client_context_id = client_request.client_context_id.clone();
        client_request.apply_service_tier();
        route.service_tier = client_request.service_tier();
        let started = Instant::now();
        let admission = authorization::PendingAdmission {
            request: client_request,
            route,
            lease,
            response_affinity_hit: selected.response_affinity_hit,
            started,
        };
        let ready = match authorization::prepare(
            &mut LiveBridge::new(downstream, upstream, runtime, key, headers, state),
            admission,
        )
        .await
        {
            authorization::Preparation::Ready(ready) => *ready,
            authorization::Preparation::Rejected => return Ok(true),
            authorization::Preparation::Closed => return Ok(false),
        };
        let authorization::ReadyAdmission {
            pending,
            authorization: prepared,
        } = ready;
        let authorization::PendingAdmission {
            request: mut client_request,
            mut route,
            lease,
            response_affinity_hit,
            started,
        } = pending;
        // Refresh may outlive a sibling, a policy edit, or a queued repair.
        // Revalidate the route before committing this still-unsent reservation.
        let (_, mut current_routes) = client_request.compatible_routes(runtime, key)?;
        let current_route = current_routes.remove(&route.candidate_id);
        if prepared.credential_fingerprint() != state.credential_fingerprint
            || prepared.incarnation() != state.authorization_incarnation
            || !state.pending_retries.is_empty()
            || current_route.as_ref().is_none_or(|current| {
                !authorization::route_matches(state, current, &client_request)
            })
        {
            drop(lease);
            authorization::defer(state, client_request);
            return Ok(true);
        }
        let mut current_route = current_route.expect("validated authorization route");
        runtime.set_route_streaming(&mut current_route, false);
        current_route.client_transport = route.client_transport;
        current_route.half_open_probe = route.half_open_probe;
        current_route.routing = route.routing.take();
        current_route.client_context_id = route.client_context_id.take();
        current_route.service_tier = route.service_tier;
        route = current_route;
        let search_activation = state.upstream_search_declared == Some(false)
            && client_request.native_search_declared_for(&route) == Some(true);
        if search_activation {
            drop(lease);
            if !state.other_requests.is_empty() {
                authorization::defer(state, client_request);
                return Ok(true);
            }
            if !client_request.materialize_connection_continuation(runtime, &key.id, &route)? {
                return Err(GatewayFailure::continuation_unavailable());
            }
            let connected = connect_upstream_while_client_connected(
                downstream,
                runtime,
                key,
                headers,
                client_request,
                false,
                0,
                None,
            )
            .await?;
            return Ok(install_connected(upstream, runtime, key, state, connected));
        }
        route.account_token_generation = prepared.token_generation;
        route.account_token_revision = prepared.token_revision.clone();
        route.observe_websocket_reuse();
        route.observe_websocket_response();
        let upstream_origin = route_error_origin(&route);
        let prepared_request =
            client_request.observed_payload_for(runtime, &mut route, headers, &prepared)?;
        client_request.budget.configure_retry_window(
            runtime.route_recovery_window_ms(),
            runtime.route_recovery_enabled(),
        );
        let dispatch = lease
            .begin_rotation_dispatch_for(&prepared, runtime)
            .map_err(|_| GatewayFailure::unavailable())?;
        let attempt = u16::try_from(dispatch.0).unwrap_or(u16::MAX);
        let event = usage_event(
            UsageAttempt {
                request_id: &client_request.request_id,
                attempt,
                local_key_id: &key.id,
                route: &route,
                reasoning_effort: Some(&prepared_request.reasoning_effort),
                requested_model: &client_request.requested_model,
                tool_use: prepared_request.tool_use,
            },
            true,
            StatusCode::OK.as_u16(),
            None,
            0,
        );
        state.upstream_origin = upstream_origin;
        state.in_flight = Some(InFlight {
            lease,
            request: client_request.clone(),
            route: route.clone(),
            event,
            usage_snapshot: None,
            started,
            response_id: None,
            prompt_affinity_key: client_request.prompt_affinity_key,
            client_visible_output: false,
            prelude: Some(super::bootstrap::PendingResponse::new(
                super::super::upstream::ConnectRecovery::reused(&route.candidate_id),
                response_affinity_hit,
                false,
            )),
            tool_call_capture: Default::default(),
            native_replay_capture: NativeReplayCapture::default(),
        });
        // Track the attempt before I/O so a failed write is audited as uncertain.
        send_request(upstream, prepared_request.payload, upstream_origin).await?;
        return Ok(true);
    }
    let connected = connect_upstream_while_client_connected(
        downstream,
        runtime,
        key,
        headers,
        client_request,
        true,
        0,
        None,
    )
    .await?;
    Ok(install_connected(upstream, runtime, key, state, connected))
}

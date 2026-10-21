use super::super::*;
use super::connection::{install_connected, send_request};
use super::terminal::{clear_transient_response_affinity, finish_incomplete};
use super::{BridgeState, InFlight};

pub(super) async fn handle_downstream_message(
    downstream: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    runtime: &GatewayRuntime,
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
            let (code, reason) = frame
                .map(|frame| {
                    (
                        UpstreamCloseCode::from(frame.code),
                        frame.reason.to_string(),
                    )
                })
                .unwrap_or((UpstreamCloseCode::Normal, String::new()));
            let _ = send_message(upstream, UpstreamMessage::Close { code, reason }).await;
            let _ = send_message(downstream, Message::Close(None)).await;
            finish_incomplete(runtime, state, error_codes::CLIENT_CANCELLED);
            return Ok(false);
        }
    }
    Ok(true)
}

async fn start_next_request(
    downstream: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    headers: &HeaderMap,
    state: &mut BridgeState,
    request_bytes: &[u8],
) -> Result<bool, GatewayFailure> {
    if state.in_flight.is_some() {
        return Err(GatewayFailure::invalid_request(
            "a response is already in progress",
        ));
    }
    let mut client_request = ClientRequest::prepare(
        runtime,
        key,
        headers,
        request_bytes,
        state
            .last_response_id
            .as_deref()
            .zip(state.transient_response_affinity_key.as_deref()),
    )
    .await?;
    if let Some(stream_id) = client_request.stream_id.as_deref() {
        if let Some(active_stream_id) = state.stream_id.as_deref() {
            if active_stream_id != stream_id {
                return Err(GatewayFailure::invalid_request(
                    "only one WebSocket stream_id is supported per connection",
                ));
            }
        } else {
            state.stream_id = Some(stream_id.to_string());
        }
    }
    let same_response_id = client_request
        .previous_response_id()
        .is_some_and(|response_id| Some(response_id) == state.last_response_id.as_deref());
    let response_affinity_key = client_request.response_affinity_key.clone();
    let model_switch_reset = same_response_id
        && !client_request.has_unpaired_tool_output()
        && response_affinity_key.as_deref().and_then(|affinity_key| {
            runtime.response_affinity_owner_supports_route(
                key,
                affinity_key,
                &client_request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                now_ms(),
            )
        }) == Some(false)
        && client_request.drop_previous_response_id(runtime, &key.id);
    if model_switch_reset {
        clear_transient_response_affinity(runtime, state);
        state.last_response_id = None;
    }
    let quota_yield = response_affinity_key
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
        })
        && client_request.yield_response_owner(runtime, &key.id);
    if same_response_id && !model_switch_reset && !quota_yield {
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
                )
                .await?;
                return Ok(
                    install_connected(downstream, upstream, runtime, key, state, connected).await,
                );
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
            return Err(GatewayFailure::unavailable());
        }
        let mut route = admitted
            .remove(&selected.candidate_id)
            .ok_or_else(GatewayFailure::unavailable)?;
        runtime.set_route_streaming(&mut route, false);
        if route.binding_key() != state.upstream_route.binding_key()
            || route.source_model != state.upstream_route.source_model
            || route.upstream_url != state.upstream_route.upstream_url
            || route.upstream_headers != state.upstream_route.upstream_headers
        {
            drop(lease);
            let connected = connect_upstream_while_client_connected(
                downstream,
                runtime,
                key,
                headers,
                client_request,
                false,
                0,
            )
            .await?;
            return Ok(
                install_connected(downstream, upstream, runtime, key, state, connected).await,
            );
        }
        let prepared = runtime
            .prepare_route_authorization(&route, now_ms())
            .await
            .map_err(|error| GatewayFailure::prepare(error, state.upstream_origin))?;
        if prepared.credential_fingerprint() != state.credential_fingerprint
            || prepared.incarnation() != state.authorization_incarnation
        {
            drop(lease);
            if !client_request.drop_previous_response_id(runtime, &key.id) {
                return Err(GatewayFailure::continuation_unavailable());
            }
            let connected = connect_upstream_while_client_connected(
                downstream,
                runtime,
                key,
                headers,
                client_request,
                true,
                0,
            )
            .await?;
            return Ok(
                install_connected(downstream, upstream, runtime, key, state, connected).await,
            );
        }
        route.client_transport = crate::UsageTransport::Websocket;
        route.half_open_probe = selected.half_open_probe;
        route.account_token_generation = prepared.token_generation;
        route.routing = Some(selected.diagnostics);
        route.observe_websocket_dispatch(None);
        route.observe_websocket_response();
        route.client_context_id = client_request.client_context_id.clone();
        client_request.apply_service_tier();
        route.service_tier = client_request.service_tier();
        let started = Instant::now();
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
        state.lease = Some(lease);
        state.upstream_origin = upstream_origin;
        state.in_flight = Some(InFlight {
            request: client_request.clone(),
            route: route.clone(),
            event,
            usage_snapshot: None,
            started,
            response_id: None,
            prompt_affinity_key: client_request.prompt_affinity_key,
            client_visible_output: false,
            legacy_call_id_repair_attempted: false,
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
    )
    .await?;
    Ok(install_connected(downstream, upstream, runtime, key, state, connected).await)
}

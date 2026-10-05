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
        Message::Ping(payload) => {
            upstream
                .send(UpstreamMessage::Ping(payload))
                .await
                .map_err(|_| GatewayFailure::transport(state.upstream_origin))?;
        }
        Message::Pong(payload) => {
            upstream
                .send(UpstreamMessage::Pong(payload))
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
            let _ = upstream.send(UpstreamMessage::Close { code, reason }).await;
            let _ = downstream.send(Message::Close(None)).await;
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
    payload: &[u8],
) -> Result<bool, GatewayFailure> {
    if state.in_flight.is_some() {
        return Err(GatewayFailure::invalid_request(
            "a response is already in progress",
        ));
    }
    let mut request = ClientRequest::parse_on_connection(
        runtime,
        key,
        headers,
        payload,
        state
            .last_response_id
            .as_deref()
            .zip(state.transient_response_affinity_key.as_deref()),
    )?;
    if let Some(stream_id) = request.stream_id.as_deref() {
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
    let same_response_id = request
        .previous_response_id()
        .is_some_and(|response_id| Some(response_id) == state.last_response_id.as_deref());
    let response_affinity_key = request.response_affinity_key.clone();
    let model_switch_reset = same_response_id
        && !request.has_unpaired_tool_output()
        && response_affinity_key.as_deref().and_then(|affinity_key| {
            runtime.response_affinity_owner_supports_route(
                key,
                affinity_key,
                &request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                now_ms(),
            )
        }) == Some(false)
        && request.drop_previous_response_id(runtime, &key.id);
    if model_switch_reset {
        clear_transient_response_affinity(runtime, state);
        state.last_response_id = None;
    }
    if same_response_id && !model_switch_reset {
        let tried = HashSet::new();
        let selected = runtime
            .select_and_reserve_with_budget(
                key,
                &request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                &tried,
                (
                    request.response_affinity_key.as_deref(),
                    request.prompt_affinity_key.as_deref(),
                ),
                now_ms(),
                &request.budget,
            )
            .await;
        let Some((selected, lease)) = selected else {
            // The owner can become temporarily ineligible while this client
            // WebSocket stays open because another chat consumed its quota.
            // Reuse would fail before reaching the owner, so reconnect through
            // the normal bounded replay path. That path materializes the local
            // Responses history and removes the opaque owner reference before
            // choosing a compatible account or API source.
            if request.has_previous_response_id() && request.requires_affinity_owner {
                let connected = connect_upstream_while_client_connected(
                    downstream, runtime, key, headers, request, false, 0,
                )
                .await?;
                return Ok(
                    install_connected(downstream, upstream, runtime, key, state, connected).await,
                );
            }
            if let Some(retry_at_ms) = runtime.earliest_retry_at(
                key,
                &request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                &tried,
                request.response_affinity_key.as_deref(),
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
        let prepared = runtime
            .prepare_authorization(&selected.candidate_id, now_ms())
            .await
            .map_err(|error| GatewayFailure::prepare(error, state.upstream_origin))?;
        if prepared.credential_fingerprint() != state.credential_fingerprint
            || prepared.incarnation() != state.authorization_incarnation
        {
            drop(lease);
            if !request.drop_previous_response_id(runtime, &key.id) {
                return Err(GatewayFailure::continuation_unavailable());
            }
            let connected = connect_upstream_while_client_connected(
                downstream, runtime, key, headers, request, true, 0,
            )
            .await?;
            return Ok(
                install_connected(downstream, upstream, runtime, key, state, connected).await,
            );
        }
        let mut route = runtime
            .executor_route(
                &selected.candidate_id,
                &request.resolved_model,
                &key.scope_snapshot(),
                WEBSOCKET_PROTOCOLS,
                false,
            )
            .ok_or_else(GatewayFailure::unavailable)?;
        route.client_transport = crate::UsageTransport::Websocket;
        route.half_open_probe = selected.half_open_probe;
        route.account_token_generation = prepared.token_generation;
        route.routing = Some(selected.diagnostics);
        route.client_context_id = client_context_fingerprint(headers);
        request.apply_service_tier_for_route(runtime, &route);
        route.service_tier = request.service_tier(runtime, &route);
        let started = Instant::now();
        let upstream_origin = route_error_origin(&route);
        let payload = request.payload_for(&route)?;
        request.budget.configure_retry_window(
            runtime.route_recovery_window_ms(),
            runtime.route_recovery_enabled(),
        );
        let dispatch = lease
            .begin_rotation_dispatch_for(&prepared, runtime)
            .map_err(|_| GatewayFailure::unavailable())?;
        let attempt = u16::try_from(dispatch.0).unwrap_or(u16::MAX);
        // A failed WebSocket flush does not prove that remote execution never
        // started. Do not convert it into a health vote or retry.
        send_request(upstream, payload, upstream_origin).await?;
        let event = usage_event(
            UsageAttempt {
                request_id: &request.request_id,
                attempt,
                local_key_id: &key.id,
                route: &route,
                reasoning_effort: Some(&request.reasoning_effort_for(&route)),
                requested_model: &request.requested_model,
                tool_use: request.tool_use_for(&route),
            },
            true,
            StatusCode::OK.as_u16(),
            None,
            0,
        );
        state.lease = Some(lease);
        state.upstream_origin = upstream_origin;
        state.in_flight = Some(InFlight {
            request: request.clone(),
            route: route.clone(),
            event,
            started,
            response_id: None,
            prompt_affinity_key: request.prompt_affinity_key,
            client_visible_output: false,
            legacy_call_id_repair_attempted: false,
            native_replay_capture: NativeReplayCapture::default(),
        });
        return Ok(true);
    }
    let connected = connect_upstream_while_client_connected(
        downstream, runtime, key, headers, request, true, 0,
    )
    .await?;
    Ok(install_connected(downstream, upstream, runtime, key, state, connected).await)
}

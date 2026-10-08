use super::super::upstream::{connect_upstream_while_client_connected, Connected};
use super::super::*;
use super::events::handle_initial_messages;
use super::terminal::clear_transient_response_affinity;
use super::{BridgeState, InFlight};

pub(super) async fn retry_upstream_connection(
    bridge: &mut super::LiveBridge<'_>,
    request: ClientRequest,
    attempt_offset: u16,
    request_id: Option<&str>,
) -> bool {
    let stream_id = request.stream_id.clone();
    match connect_upstream_while_client_connected(
        bridge.downstream,
        bridge.runtime,
        bridge.key,
        bridge.headers,
        request,
        true,
        attempt_offset,
    )
    .await
    {
        Ok(connected) => {
            install_connected(
                bridge.downstream,
                bridge.upstream,
                bridge.runtime,
                bridge.key,
                bridge.bridge_state,
                connected,
            )
            .await
        }
        Err(retry_failure) => {
            send_gateway_error(
                bridge.downstream,
                &retry_failure,
                request_id,
                stream_id.as_deref(),
            )
            .await;
            false
        }
    }
}

pub(in crate::gateway::websocket) async fn send_request(
    upstream: &mut UpstreamWebSocket,
    request_payload: String,
    origin: ErrorOrigin,
) -> Result<(), GatewayFailure> {
    let send = async {
        upstream
            .send(UpstreamMessage::Text(request_payload))
            .await?;
        upstream.flush().await
    };
    match timeout(UPSTREAM_CONNECT_TIMEOUT, send).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(GatewayFailure::transport(origin)),
    }
}

pub(super) async fn install_connected(
    downstream: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    state: &mut BridgeState,
    connected: Connected,
) -> bool {
    let Connected {
        credential_fingerprint,
        authorization_incarnation,
        upstream: next_upstream,
        initial_messages,
        route,
        request,
        lease,
        attempt,
        started,
    } = connected;
    let _ = upstream
        .send(UpstreamMessage::Close {
            code: UpstreamCloseCode::Normal,
            reason: String::new(),
        })
        .await;
    *upstream = next_upstream;
    install_in_flight(
        runtime,
        state,
        key,
        InFlightInstall {
            credential_fingerprint,
            authorization_incarnation,
            request,
            route,
            lease,
            attempt,
            started,
        },
    );
    handle_initial_messages(downstream, runtime, state, initial_messages).await
}

pub(super) struct InFlightInstall {
    pub(super) credential_fingerprint: [u8; 32],
    pub(super) authorization_incarnation: AuthorizationIncarnation,
    pub(super) request: ClientRequest,
    pub(super) route: ExecutorRoute,
    pub(super) lease: CandidateLease,
    pub(super) attempt: u16,
    pub(super) started: Instant,
}

pub(super) fn install_in_flight(
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    key: &AuthenticatedKey,
    install: InFlightInstall,
) {
    let InFlightInstall {
        credential_fingerprint,
        authorization_incarnation,
        request,
        route,
        lease,
        attempt,
        started,
    } = install;
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
    clear_transient_response_affinity(runtime, state);
    state.lease = Some(lease);
    state.upstream_candidate_id = route.candidate_id.clone();
    state.credential_fingerprint = credential_fingerprint;
    state.authorization_incarnation = authorization_incarnation;
    state.upstream_origin = route_error_origin(&route);
    state.last_response_id = None;
    state.in_flight = Some(InFlight {
        request: request.clone(),
        route,
        event,
        started,
        response_id: None,
        prompt_affinity_key: request.prompt_affinity_key,
        client_visible_output: false,
        legacy_call_id_repair_attempted: false,
        native_replay_capture: NativeReplayCapture::default(),
    });
}

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
    send_message(upstream, UpstreamMessage::Text(request_payload))
        .await
        .map_err(|_| GatewayFailure::transport(origin))
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
        reasoning_effort,
        tool_use,
        lease,
        attempt,
        started,
    } = connected;
    let _ = send_message(
        upstream,
        UpstreamMessage::Close {
            code: UpstreamCloseCode::Normal,
            reason: String::new(),
        },
    )
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
            reasoning_effort,
            tool_use,
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
    pub(super) reasoning_effort: crate::usage::ReasoningEffortDiagnostics,
    pub(super) tool_use: crate::ToolUseDiagnostics,
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
        reasoning_effort,
        tool_use,
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
            reasoning_effort: Some(&reasoning_effort),
            requested_model: &request.requested_model,
            tool_use,
        },
        true,
        StatusCode::OK.as_u16(),
        None,
        0,
    );
    clear_transient_response_affinity(runtime, state);
    state.lease = Some(lease);
    state.upstream_candidate_id = route.candidate_id.clone();
    state.upstream_responses_lite = route.account_id.is_some() && request.responses_lite;
    state.upstream_route = route.clone();
    state.credential_fingerprint = credential_fingerprint;
    state.authorization_incarnation = authorization_incarnation;
    state.upstream_origin = route_error_origin(&route);
    state.last_response_id = None;
    state.in_flight = Some(InFlight {
        request: request.clone(),
        route,
        event,
        usage_snapshot: None,
        started,
        response_id: None,
        prompt_affinity_key: request.prompt_affinity_key,
        client_visible_output: false,
        legacy_call_id_repair_attempted: false,
        tool_call_capture: Default::default(),
        native_replay_capture: NativeReplayCapture::default(),
    });
}

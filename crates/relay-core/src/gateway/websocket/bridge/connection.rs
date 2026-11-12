use super::super::upstream::{
    Connected, ConnectedTransport, connect_transport_while_client_connected,
};
use super::super::*;
use super::terminal::{clear_connection_responses, finish_incomplete};
use super::{BridgeState, ConnectionState, InFlight};

pub(super) use super::super::upstream::HttpFallback as HttpHandoff;

pub(super) async fn finish_connection_failure(
    downstream: &mut WebSocket,
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    failure: &GatewayFailure,
) {
    let mut owners = state
        .active_requests()
        .map(|request| {
            (
                request.request.request_id.clone(),
                request.request.stream_id.clone(),
                request.client_visible_output,
            )
        })
        .collect::<Vec<_>>();
    if let Some(pending) = state.pending_admission.as_ref() {
        owners.push((
            pending.request.request_id.clone(),
            pending.request.stream_id.clone(),
            false,
        ));
    }
    finish_incomplete(runtime, state, failure.category);
    for (request_id, lane, visible) in &owners {
        if !visible
            && super::super::failure::send_request_error(
                downstream,
                failure,
                Some(request_id),
                lane.as_deref(),
            )
            .await
            .is_err()
        {
            return;
        }
    }
    super::super::io::close_downstream(
        downstream,
        Message::Close(Some(CloseFrame {
            code: close_code::ERROR,
            reason: "upstream connection ended".into(),
        })),
    )
    .await;
}

pub(super) async fn retry_upstream_connection(
    bridge: &mut super::LiveBridge<'_>,
    mut request: ClientRequest,
    attempt_offset: u16,
    request_id: Option<&str>,
    recovery: super::super::upstream::ConnectRecovery,
) -> bool {
    if !bridge.bridge_state.other_requests.is_empty()
        || bridge.bridge_state.pending_admission.is_some()
    {
        bridge.bridge_state.pending_retries.push_back((
            super::RetryHandoff {
                request_id: request_id.map(str::to_owned),
                attempt_offset,
                recovery,
            },
            request,
        ));
        return true;
    }
    request
        .websocket_authorization
        .get_or_insert_with(|| bridge.bridge_state.websocket_authorization());
    let stream_id = request.stream_id.clone();
    match connect_transport_while_client_connected(
        bridge.downstream,
        bridge.runtime,
        bridge.key,
        bridge.headers,
        request,
        recovery.allow_previous_response_reset,
        attempt_offset,
        Some(recovery),
    )
    .await
    {
        Ok(ConnectedTransport::Websocket(connected)) => install_connected(
            bridge.upstream,
            bridge.runtime,
            bridge.key,
            bridge.bridge_state,
            *connected,
        ),
        Ok(ConnectedTransport::Http(fallback)) => {
            bridge.bridge_state.http_handoff = Some(*fallback);
            true
        }
        Err(retry_failure) => super::super::failure::send_request_error(
            bridge.downstream,
            &retry_failure,
            request_id,
            stream_id.as_deref(),
        )
        .await
        .is_ok(),
    }
}

pub(in crate::gateway::websocket) async fn send_request(
    upstream: &mut UpstreamWebSocket,
    request_payload: String,
    origin: ErrorOrigin,
) -> Result<(), GatewayFailure> {
    upstream
        .send_request(request_payload)
        .await
        .map_err(|_| GatewayFailure::transport(origin))
}

pub(super) fn install_connected(
    upstream: &mut UpstreamWebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    state: &mut BridgeState,
    connected: Connected,
) -> bool {
    debug_assert_eq!(
        state.active_count(),
        0,
        "socket replacement requires drained owners"
    );
    let Connected {
        credential_fingerprint,
        authorization_incarnation,
        native_search_declared,
        upstream: next_upstream,
        recovery,
        response_affinity_hit,
        route,
        request,
        reasoning_effort,
        tool_use,
        lease,
        attempt,
        started,
    } = connected;
    // The retired connection must not delay a ready replacement while its
    // peer is stalled. Dropping it does not replay either generation.
    *upstream = next_upstream;
    install_in_flight(
        runtime,
        state,
        key,
        InFlightInstall {
            credential_fingerprint,
            authorization_incarnation,
            native_search_declared,
            request,
            route,
            reasoning_effort,
            tool_use,
            lease,
            attempt,
            started,
            recovery,
            response_affinity_hit,
        },
    );
    true
}

pub(super) struct InFlightInstall {
    pub(super) credential_fingerprint: [u8; 32],
    pub(super) authorization_incarnation: AuthorizationIncarnation,
    pub(super) native_search_declared: Option<bool>,
    pub(super) request: ClientRequest,
    pub(super) route: ExecutorRoute,
    pub(super) reasoning_effort: crate::usage::ReasoningEffortDiagnostics,
    pub(super) tool_use: crate::ToolUseDiagnostics,
    pub(super) lease: CandidateLease,
    pub(super) attempt: u16,
    pub(super) started: Instant,
    pub(super) recovery: super::super::upstream::ConnectRecovery,
    pub(super) response_affinity_hit: bool,
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
        native_search_declared,
        request,
        route,
        reasoning_effort,
        tool_use,
        lease,
        attempt,
        started,
        recovery,
        response_affinity_hit,
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
    clear_connection_responses(runtime, state);
    state.connection = ConnectionState::Native;
    state.upstream_candidate_id = route.candidate_id.clone();
    state.upstream_responses_lite = route.account_id.is_some() && request.responses_lite;
    state.upstream_search_declared = native_search_declared;
    state.upstream_route = route.clone();
    state.credential_fingerprint = credential_fingerprint;
    state.authorization_incarnation = authorization_incarnation;
    state.upstream_origin = route_error_origin(&route);
    state.in_flight = Some(InFlight {
        lease,
        request: request.clone(),
        route,
        event,
        usage_snapshot: None,
        started,
        response_id: None,
        prompt_affinity_key: request.prompt_affinity_key,
        client_visible_output: false,
        prelude: Some(super::bootstrap::PendingResponse::new(
            recovery,
            response_affinity_hit,
            true,
        )),
        tool_call_capture: Default::default(),
        native_replay_capture: NativeReplayCapture::default(),
    });
}

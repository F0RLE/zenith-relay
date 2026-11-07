use super::super::upstream::{
    ConnectPhase, ConnectRecovery, ConnectScope, TerminalAction, handle_initial_terminal,
    initial_message_state, initial_messages_are_empty_incomplete, message_serves_rejected_model,
    record_connect_failure,
};
use super::super::*;
use super::events::{forward_upstream_message, inspect_upstream_event};
use super::{BridgeState, InFlight, LiveBridge, RetryHandoff};

const MAX_SETUP_EVENTS: usize = 1024;

enum SetupDelivery {
    Buffered {
        messages: Vec<UpstreamMessage>,
        bytes: usize,
    },
    Forward,
}

pub(super) struct PendingResponse {
    recovery: ConnectRecovery,
    response_affinity_hit: bool,
    setup: SetupDelivery,
}

impl PendingResponse {
    pub(super) fn new(
        recovery: ConnectRecovery,
        response_affinity_hit: bool,
        buffer: bool,
    ) -> Self {
        Self {
            recovery,
            response_affinity_hit,
            setup: if buffer {
                SetupDelivery::Buffered {
                    messages: Vec::new(),
                    bytes: 0,
                }
            } else {
                SetupDelivery::Forward
            },
        }
    }

    async fn flush(self, bridge: &mut LiveBridge<'_>) -> bool {
        if let SetupDelivery::Buffered { messages, .. } = self.setup {
            for message in messages {
                if !forward_upstream_message(
                    bridge.downstream,
                    bridge.runtime,
                    bridge.bridge_state,
                    message,
                )
                .await
                {
                    return false;
                }
            }
        }
        true
    }
}

fn scope<'a>(
    bridge: &'a mut InFlight,
    runtime: &'a GatewayRuntime,
    key: &'a AuthenticatedKey,
    affinity_hit: bool,
) -> ConnectScope<'a> {
    ConnectScope {
        event: Some(&bridge.event),
        runtime,
        key,
        request: &mut bridge.request,
        route: &bridge.route,
        lease: &bridge.lease,
        attempt: bridge.event.attempt,
        started: bridge.started,
        source_error_origin: route_error_origin(&bridge.route),
        response_affinity_hit: affinity_hit,
    }
}

fn retire_parent(
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    request: &InFlight,
    previous: Option<&str>,
) {
    let retired = state
        .responses
        .retire_failed_continuation(request.request.stream_id.as_deref(), previous);
    runtime.invalidate_response_affinity(retired.as_deref());
}

async fn retry(
    bridge: &mut LiveBridge<'_>,
    pending: PendingResponse,
    previous: Option<&str>,
) -> bool {
    let Some(request) = bridge.bridge_state.in_flight.take() else {
        return false;
    };
    retire_parent(bridge.runtime, bridge.bridge_state, &request, previous);
    let InFlight {
        request,
        event,
        lease,
        ..
    } = request;
    drop(lease);
    let PendingResponse { recovery, .. } = pending;
    let handoff = RetryHandoff {
        request_id: Some(request.request_id.clone()),
        attempt_offset: event.attempt,
        recovery,
    };
    handoff.retry(bridge, request).await
}

async fn request_error(
    bridge: &mut LiveBridge<'_>,
    failure: &GatewayFailure,
    request_id: &str,
    lane: Option<&str>,
) -> bool {
    super::super::failure::send_request_error(bridge.downstream, failure, Some(request_id), lane)
        .await
        .is_ok()
}

async fn reject(bridge: &mut LiveBridge<'_>, failure: GatewayFailure) -> bool {
    let Some(request) = bridge.bridge_state.in_flight.take() else {
        return false;
    };
    retire_parent(
        bridge.runtime,
        bridge.bridge_state,
        &request,
        request.request.previous_response_id(),
    );
    let id = request.request.request_id.clone();
    let lane = request.request.stream_id.clone();
    drop(request);
    request_error(bridge, &failure, &id, lane.as_deref()).await
}

/// The socket owns admission immediately after dispatch; pre-output recovery
/// still owns one request, its budget, setup frames and known usage.
pub(super) async fn handle_message(bridge: &mut LiveBridge<'_>, message: UpstreamMessage) -> bool {
    let bytes = match &message {
        UpstreamMessage::Text(text) => text.as_bytes(),
        UpstreamMessage::Binary(bytes) => bytes.as_ref(),
        _ => {
            return forward_upstream_message(
                bridge.downstream,
                bridge.runtime,
                bridge.bridge_state,
                message,
            )
            .await;
        }
    };
    let Some(mut pending) = bridge
        .bridge_state
        .in_flight
        .as_mut()
        .and_then(|request| request.prelude.take())
    else {
        return forward_upstream_message(
            bridge.downstream,
            bridge.runtime,
            bridge.bridge_state,
            message,
        )
        .await;
    };
    let (generation, terminal) = initial_message_state(&message);
    if generation || bytes.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
        if !pending.flush(bridge).await {
            return false;
        }
        return forward_upstream_message(
            bridge.downstream,
            bridge.runtime,
            bridge.bridge_state,
            message,
        )
        .await;
    }
    let runtime = bridge.runtime;
    let key = bridge.key;
    let previous = bridge
        .bridge_state
        .in_flight
        .as_ref()
        .and_then(|request| request.request.previous_response_id())
        .map(str::to_owned);
    let degraded = runtime.block_degraded_routes_enabled()
        && bridge
            .bridge_state
            .in_flight
            .as_ref()
            .is_some_and(|request| {
                request.route.account_id.is_some()
                    && message_serves_rejected_model(&message, &request.route.source_model)
            });
    if degraded {
        inspect_upstream_event(bytes, bridge.bridge_state);
        let failure = GatewayFailure::classified(
            StatusCode::NOT_FOUND,
            error_codes::UPSTREAM_ROUTE_DEGRADED,
            bridge.bridge_state.upstream_origin,
        );
        let attempt = bridge.bridge_state.in_flight.as_mut().unwrap();
        record_connect_failure(
            &scope(attempt, runtime, key, pending.response_affinity_hit).trace(),
            &failure,
            None,
        );
        *pending.recovery.progress().last_failure = Some(failure);
        return retry(bridge, pending, previous.as_deref()).await;
    }
    if terminal.outcome == Some(EventTerminalOutcome::Failure) {
        let terminal = inspect_upstream_event(bytes, bridge.bridge_state);
        let action = {
            let attempt = bridge.bridge_state.in_flight.as_mut().unwrap();
            handle_initial_terminal(
                std::slice::from_ref(&message),
                &mut scope(attempt, runtime, key, pending.response_affinity_hit),
                &mut pending.recovery.progress(),
            )
        };
        match action {
            Ok(action @ (TerminalAction::Continue | TerminalAction::Break)) => {
                if matches!(action, TerminalAction::Break) {
                    pending.recovery.phase = ConnectPhase::Finish;
                }
                if let Some(attempt) = bridge.bridge_state.in_flight.as_ref() {
                    runtime.observe_codex_quota_headers(
                        &attempt.route.candidate_id,
                        attempt.route.account_token_generation,
                        attempt.route.account_token_revision.as_ref(),
                        terminal.status.unwrap_or(StatusCode::BAD_GATEWAY),
                        &terminal.headers,
                        now_ms(),
                    );
                }
                return retry(bridge, pending, previous.as_deref()).await;
            }
            Err(failure) => {
                let request = bridge.bridge_state.in_flight.as_ref().unwrap();
                let id = request.request.request_id.clone();
                let lane = request.request.stream_id.clone();
                super::terminal::finish_terminal(runtime, bridge.bridge_state, terminal);
                return request_error(bridge, &failure, &id, lane.as_deref()).await;
            }
            Ok(TerminalAction::Proceed) => {}
        }
    }
    if terminal.outcome == Some(EventTerminalOutcome::Incomplete)
        && initial_messages_are_empty_incomplete(std::slice::from_ref(&message))
    {
        inspect_upstream_event(bytes, bridge.bridge_state);
        let failure = GatewayFailure::classified(
            StatusCode::BAD_GATEWAY,
            error_codes::STREAM_INCOMPLETE,
            bridge.bridge_state.upstream_origin,
        );
        let attempt = bridge.bridge_state.in_flight.as_mut().unwrap();
        attempt.lease.settle_rotation_terminal(now_ms());
        record_connect_failure(
            &scope(attempt, runtime, key, pending.response_affinity_hit).trace(),
            &failure,
            None,
        );
        return reject(bridge, failure).await;
    }
    if terminal.outcome.is_some() {
        if !pending.flush(bridge).await {
            return false;
        }
        return forward_upstream_message(bridge.downstream, runtime, bridge.bridge_state, message)
            .await;
    }
    match &mut pending.setup {
        SetupDelivery::Buffered {
            messages,
            bytes: retained,
        } => {
            super::events::observe_buffered_setup(bytes, bridge.bridge_state);
            if messages.len() >= MAX_SETUP_EVENTS
                || bytes.len()
                    > MAX_WEBSOCKET_MESSAGE_BYTES
                        .saturating_mul(2)
                        .saturating_sub(*retained)
            {
                let mut failure =
                    GatewayFailure::message_too_large(bridge.bridge_state.upstream_origin);
                failure.message = "upstream WebSocket startup buffer exceeded Relay limits";
                super::connection::finish_connection_failure(
                    bridge.downstream,
                    runtime,
                    bridge.bridge_state,
                    &failure,
                )
                .await;
                return false;
            }
            *retained += bytes.len();
            messages.push(message);
            bridge.bridge_state.in_flight.as_mut().unwrap().prelude = Some(pending);
            true
        }
        SetupDelivery::Forward => {
            bridge.bridge_state.in_flight.as_mut().unwrap().prelude = Some(pending);
            let keep =
                forward_upstream_message(bridge.downstream, runtime, bridge.bridge_state, message)
                    .await;
            if let Some(request) = bridge.bridge_state.in_flight.as_mut()
                && request.client_visible_output
            {
                request.prelude.take();
            }
            keep
        }
    }
}

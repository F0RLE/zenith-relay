use std::collections::HashSet;
use std::time::Instant;

use super::super::super::execution::AttemptRepairs;
use super::super::bridge::send_request;
use super::super::*;
use super::initial_terminal::{handle_initial_terminal, TerminalAction};
use super::messages::{initial_application_messages, initial_messages_are_empty_incomplete};
use super::telemetry::record_connect_failure;
use super::{ConnectProgress, ConnectScope, ConnectTrace, Connected};
use crate::runtime::PreparedAuthorization;

#[allow(clippy::large_enum_variant)]
pub(super) enum OpenedSocket {
    Continue(ClientRequest),
    Break(ClientRequest),
    Ready(Connected),
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn open_upgraded_socket(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    mut request: ClientRequest,
    route: ExecutorRoute,
    lease: CandidateLease,
    upgrade: reqwest_websocket::UpgradeResponse,
    payload: String,
    prepared: PreparedAuthorization,
    attempt: &mut u16,
    started: Instant,
    source_error_origin: ErrorOrigin,
    response_affinity_hit: bool,
    tried: &mut HashSet<String>,
    repairs: &mut AttemptRepairs,
    confirmed_response_missing: &mut bool,
    last_failure: &mut Option<GatewayFailure>,
    http_fallback_origin: &mut Option<ErrorOrigin>,
) -> Result<OpenedSocket, GatewayFailure> {
    let Ok(Ok(mut upstream)) = timeout(UPSTREAM_CONNECT_TIMEOUT, upgrade.into_websocket()).await
    else {
        let failure = GatewayFailure::transport(source_error_origin);
        record_connect_failure(
            &ConnectTrace {
                runtime,
                lease: &lease,
                key,
                route: &route,
                request: &request,
                attempt: *attempt,
                started,
            },
            &failure,
            None,
        );
        *last_failure = Some(failure);
        return Ok(OpenedSocket::Continue(request));
    };
    runtime.mark_websocket_supported(&route.candidate_id, &request.resolved_model);
    // WebSocket upgrade/auth probes are not generation dispatches. The
    // actual payload send is; all reconnect and HTTP fallback paths share
    // this same counter through ClientRequest.
    let dispatch = lease
        .begin_rotation_dispatch_for(&prepared, runtime)
        .map_err(|_| GatewayFailure::unavailable())?;
    *attempt = u16::try_from(dispatch.0).unwrap_or(u16::MAX);
    if send_request(&mut upstream, payload, source_error_origin)
        .await
        .is_err()
    {
        lease.settle_rotation_unknown(now_ms());
        let failure = GatewayFailure::transport(source_error_origin);
        // The frame may have reached the provider even when flush fails.
        // Do not cool the route or replay an unknown outcome.
        return Err(failure);
    }
    let initial_messages =
        match initial_application_messages(&mut upstream, source_error_origin).await {
            Ok(messages) => messages,
            Err(failure) => {
                lease.settle_rotation_unknown(now_ms());
                let response_headers = HeaderMap::new();
                record_connect_failure(
                    &ConnectTrace {
                        runtime,
                        lease: &lease,
                        key,
                        route: &route,
                        request: &request,
                        attempt: *attempt,
                        started,
                    },
                    &failure,
                    Some(&response_headers),
                );
                return Err(failure);
            }
        };
    if initial_messages_are_empty_incomplete(&initial_messages) {
        lease.settle_rotation_terminal(now_ms());
        let failure = GatewayFailure::classified(
            StatusCode::BAD_GATEWAY,
            error_codes::STREAM_INCOMPLETE,
            source_error_origin,
        );
        record_connect_failure(
            &ConnectTrace {
                runtime,
                lease: &lease,
                key,
                route: &route,
                request: &request,
                attempt: *attempt,
                started,
            },
            &failure,
            None,
        );
        return Err(failure);
    }
    match handle_initial_terminal(
        &initial_messages,
        &mut ConnectScope {
            runtime,
            key,
            request: &mut request,
            route: &route,
            lease: &lease,
            attempt: *attempt,
            started,
            source_error_origin,
            response_affinity_hit,
        },
        &mut ConnectProgress {
            tried,
            repairs,
            confirmed_response_missing,
            last_failure,
            http_fallback_origin,
        },
    )? {
        TerminalAction::Continue => return Ok(OpenedSocket::Continue(request)),
        TerminalAction::Break => return Ok(OpenedSocket::Break(request)),
        TerminalAction::Proceed => {}
    }
    Ok(OpenedSocket::Ready(Connected {
        credential_fingerprint: prepared.credential_fingerprint(),
        authorization_incarnation: prepared.incarnation(),
        upstream,
        initial_messages,
        route,
        request,
        lease,
        attempt: *attempt,
        started,
    }))
}

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
    request_payload: String,
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
    runtime.mark_websocket_supported(
        &GatewayRuntime::websocket_capability_scope(&route, &prepared),
        &request.resolved_model,
        &prepared.incarnation(),
    );
    // WebSocket upgrade/auth probes are not generation dispatches. The
    // actual payload send is; all reconnect and HTTP fallback paths share
    // this same counter through ClientRequest.
    let dispatch = lease
        .begin_rotation_dispatch_for(&prepared, runtime)
        .map_err(|_| GatewayFailure::unavailable())?;
    *attempt = u16::try_from(dispatch.0).unwrap_or(u16::MAX);
    let mut pending_usage = PendingConnectUsage {
        runtime,
        event: Some(usage_event(
            UsageAttempt {
                request_id: &request.request_id,
                attempt: *attempt,
                local_key_id: &key.id,
                route: &route,
                reasoning_effort: Some(&request.reasoning_effort_for(&route)),
                requested_model: &request.requested_model,
                tool_use: request.tool_use_for(&route),
            },
            false,
            200,
            Some(error_codes::CLIENT_CANCELLED.to_string()),
            0,
        )),
        started,
    };
    if send_request(&mut upstream, request_payload, source_error_origin)
        .await
        .is_err()
    {
        lease.settle_rotation_unknown(now_ms());
        let failure = GatewayFailure::transport(source_error_origin);
        pending_usage.finish(failure.category, crate::AttemptOutcome::ExecutionUncertain);
        // The frame may have reached the provider even when flush fails.
        // Do not cool the route or replay an unknown outcome.
        return Err(failure);
    }
    let block_degraded_routes =
        route.account_id.is_some() && runtime.block_degraded_routes_enabled();
    let initial_result = initial_application_messages(
        &mut upstream,
        source_error_origin,
        block_degraded_routes.then_some(route.source_model.as_str()),
    )
    .await;
    // Normal completion is handed to the existing terminal/bridge observers.
    // Dropping the future before this point must still audit the cancellation.
    pending_usage.event.take();
    let initial_messages = match initial_result {
        Ok(messages) => messages,
        Err(failure) if failure.category == error_codes::UPSTREAM_ROUTE_DEGRADED => {
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
        }
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

struct PendingConnectUsage<'a> {
    runtime: &'a GatewayRuntime,
    event: Option<UsageEvent>,
    started: Instant,
}

impl PendingConnectUsage<'_> {
    fn finish(&mut self, category: &str, outcome: crate::AttemptOutcome) {
        let Some(mut event) = self.event.take() else {
            return;
        };
        event.error_category = Some(category.to_string());
        event.latency_ms = self.started.elapsed().as_millis() as u64;
        if let Some(routing) = event.routing.as_mut() {
            routing.attempt_outcome = Some(outcome);
        }
        emit_usage(self.runtime, event);
    }
}

impl Drop for PendingConnectUsage<'_> {
    fn drop(&mut self) {
        self.finish(
            error_codes::CLIENT_CANCELLED,
            crate::AttemptOutcome::ClientDisconnected,
        );
    }
}

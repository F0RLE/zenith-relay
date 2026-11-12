use std::time::Instant;

use super::super::bridge::send_request;
use super::super::*;
use super::telemetry::record_connect_failure;
use super::{ConnectRecovery, ConnectTrace, Connected};
use crate::runtime::PreparedAuthorization;

#[allow(clippy::large_enum_variant)]
pub(super) enum OpenedSocket {
    Continue(ClientRequest),
    Ready(Connected),
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn open_upgraded_socket(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    mut request: ClientRequest,
    route: ExecutorRoute,
    lease: CandidateLease,
    upgrade: super::super::transport::UpgradeResponse,
    prepared_request: PreparedPayload,
    prepared: PreparedAuthorization,
    attempt: &mut u16,
    started: Instant,
    source_error_origin: ErrorOrigin,
    response_affinity_hit: bool,
    last_failure: &mut Option<GatewayFailure>,
) -> Result<OpenedSocket, GatewayFailure> {
    let Ok(Ok(mut upstream)) = timeout(UPSTREAM_CONNECT_TIMEOUT, upgrade.into_websocket()).await
    else {
        let failure = GatewayFailure::transport(source_error_origin);
        record_connect_failure(
            &ConnectTrace {
                event: None,
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
    request.observe_websocket_authorization(&route.candidate_id, &prepared);
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
                reasoning_effort: Some(&prepared_request.reasoning_effort),
                requested_model: &request.requested_model,
                tool_use: prepared_request.tool_use.clone(),
            },
            false,
            200,
            Some(error_codes::CLIENT_CANCELLED.to_string()),
            0,
        )),
        started,
    };
    if send_request(&mut upstream, prepared_request.payload, source_error_origin)
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
    // Transfer the sent attempt and its lease without waiting for application
    // output. The bridge can admit another lane while this response starts.
    pending_usage.event.take();
    Ok(OpenedSocket::Ready(Connected {
        credential_fingerprint: prepared.credential_fingerprint(),
        authorization_incarnation: prepared.incarnation(),
        native_search_declared: prepared_request.native_search_declared,
        upstream,
        recovery: ConnectRecovery::new(true),
        response_affinity_hit,
        route,
        request,
        reasoning_effort: prepared_request.reasoning_effort,
        tool_use: prepared_request.tool_use,
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

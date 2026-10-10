use std::collections::HashSet;
use std::time::Instant;

use super::super::super::execution::AttemptRepairs;
use super::super::*;
use super::handshake::{upgrade_with_authorization_refresh, AuthorizationUpgrade};
use super::open::{open_upgraded_socket, OpenedSocket};
use super::telemetry::record_connect_failure;
use super::upgrade_reject::{handle_upgrade_rejection, UpgradeAction};
use super::{ConnectProgress, ConnectScope, ConnectTrace, Connected};
use crate::runtime::AccountTransport;

#[allow(clippy::large_enum_variant)]
pub(super) enum DrivenConnect {
    Continue(ClientRequest),
    Break(ClientRequest),
    Ready(Connected),
}

pub(super) struct DriveCandidateInput<'a> {
    pub(super) selected: crate::Selection,
    pub(super) lease: CandidateLease,
    pub(super) request: ClientRequest,
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) client_headers: &'a HeaderMap,
    pub(super) attempt: &'a mut u16,
    pub(super) tried: &'a mut HashSet<String>,
    pub(super) repairs: &'a mut AttemptRepairs,
    pub(super) confirmed_response_missing: &'a mut bool,
    pub(super) last_failure: &'a mut Option<GatewayFailure>,
    pub(super) http_fallback_origin: &'a mut Option<ErrorOrigin>,
}

/// Drive one reserved WebSocket candidate through upgrade. Continue and break
/// hand the same request back; a ready socket consumes it.
pub(super) async fn drive_selected_candidate(
    connect_input: DriveCandidateInput<'_>,
) -> Result<DrivenConnect, GatewayFailure> {
    let DriveCandidateInput {
        selected,
        lease,
        mut request,
        runtime,
        key,
        client_headers,
        attempt,
        tried,
        repairs,
        confirmed_response_missing,
        last_failure,
        http_fallback_origin: websocket_http_fallback_origin,
    } = connect_input;
    tried.insert(selected.candidate_id.clone());
    let response_affinity_hit = selected.response_affinity_hit;
    let Some(mut route) = runtime.executor_route(
        &selected.candidate_id,
        &request.resolved_model,
        &key.scope_snapshot(),
        WEBSOCKET_PROTOCOLS,
        false,
    ) else {
        return Ok(DrivenConnect::Continue(request));
    };
    route.client_transport = crate::UsageTransport::Websocket;
    request.apply_service_tier_for_route(runtime, &route);
    route.service_tier = request.service_tier(runtime, &route);
    route.half_open_probe = selected.half_open_probe;
    route.routing = Some(selected.diagnostics);
    route.client_context_id = client_context_fingerprint(client_headers);
    let source_error_origin = route_error_origin(&route);
    if route.client_wire_api != WireApi::Responses {
        return Ok(DrivenConnect::Continue(request));
    }
    // Basis Points speaks HTTP on its responses URL. Upgrading that URL as a
    // native Responses socket is rejected with 502 Invalid request body, so
    // this client socket must go through the HTTP executor instead.
    let basis_points = route.account_transport == AccountTransport::ExcelBasisPoints;
    if basis_points || !route.adapter.is_passthrough() {
        if !basis_points {
            runtime.mark_websocket_http_only(
                &route.candidate_id,
                &request.resolved_model,
                now_ms(),
            );
        }
        drop(lease);
        *websocket_http_fallback_origin = Some(source_error_origin);
        *last_failure = Some(GatewayFailure::websocket_http_fallback(source_error_origin));
        return Ok(if basis_points {
            DrivenConnect::Break(request)
        } else {
            DrivenConnect::Continue(request)
        });
    }
    if runtime.websocket_is_http_only(&route.candidate_id, &request.resolved_model, now_ms()) {
        drop(lease);
        *websocket_http_fallback_origin = Some(source_error_origin);
        *last_failure = Some(GatewayFailure::websocket_http_fallback(source_error_origin));
        return Ok(DrivenConnect::Continue(request));
    }
    let Some(_wire_attempt) = request.budget.start_wire_attempt() else {
        drop(lease);
        return Ok(DrivenConnect::Break(request));
    };
    let started = Instant::now();
    // Reads the current attempt. The dispatch number changes after the request is sent.
    macro_rules! connect_trace {
        () => {
            &ConnectTrace {
                runtime,
                lease: &lease,
                key,
                route: &route,
                request: &request,
                attempt: *attempt,
                started,
            }
        };
    }
    let prepared = match runtime
        .prepare_authorization(&route.candidate_id, now_ms())
        .await
    {
        Ok(prepared) => prepared,
        Err(error) => {
            let failure = GatewayFailure::prepare(error, source_error_origin);
            record_connect_failure(connect_trace!(), &failure, None);
            *last_failure = Some(failure);
            return Ok(DrivenConnect::Continue(request));
        }
    };
    let request_payload = request.observed_payload_for(runtime, &mut route)?;
    let mut prepared = prepared;
    let upgrade = match upgrade_with_authorization_refresh(
        runtime,
        client_headers,
        key,
        &route,
        &request,
        &lease,
        *attempt,
        started,
        source_error_origin,
        &mut prepared,
        last_failure,
    )
    .await
    {
        AuthorizationUpgrade::ContinueCandidate => return Ok(DrivenConnect::Continue(request)),
        AuthorizationUpgrade::Upgraded(upgrade) => upgrade,
    };
    // The final prepared authorization may differ from the first attempt
    // after an in-band 401 refresh. Preserve its generation on this
    // request-local route so every later WebSocket usage event is tied to
    // the credential that actually performed the upgrade.
    route.account_token_generation = prepared.token_generation;
    let status = upgrade.status();
    let response_headers = upgrade.headers().clone();
    runtime.observe_codex_quota_headers(&route.candidate_id, status, &response_headers, now_ms());
    if status == StatusCode::SWITCHING_PROTOCOLS {
        let turn_scope = request_scope(
            &key.id,
            client_headers,
            route.account_id.as_deref(),
            &route.source_model,
        );
        runtime.observe_turn_state(&response_headers, turn_scope.as_ref(), &prepared);
    }
    if status != StatusCode::SWITCHING_PROTOCOLS {
        let mut scope = ConnectScope {
            runtime,
            key,
            request: &mut request,
            route: &route,
            lease: &lease,
            attempt: *attempt,
            started,
            source_error_origin,
            response_affinity_hit,
        };
        let mut progress = ConnectProgress {
            tried,
            repairs,
            confirmed_response_missing,
            last_failure,
            http_fallback_origin: websocket_http_fallback_origin,
        };
        match handle_upgrade_rejection(
            upgrade,
            status,
            &response_headers,
            &mut scope,
            &mut progress,
        )
        .await?
        {
            UpgradeAction::ContinueCandidates => return Ok(DrivenConnect::Continue(request)),
            UpgradeAction::Break => return Ok(DrivenConnect::Break(request)),
            UpgradeAction::Fail(failure) => return Err(failure),
        }
    }
    match open_upgraded_socket(
        runtime,
        key,
        request,
        route,
        lease,
        upgrade,
        request_payload,
        prepared,
        attempt,
        started,
        source_error_origin,
        response_affinity_hit,
        tried,
        repairs,
        confirmed_response_missing,
        last_failure,
        websocket_http_fallback_origin,
    )
    .await?
    {
        OpenedSocket::Continue(returned) => Ok(DrivenConnect::Continue(returned)),
        OpenedSocket::Break(returned) => Ok(DrivenConnect::Break(returned)),
        OpenedSocket::Ready(connected) => Ok(DrivenConnect::Ready(connected)),
    }
}

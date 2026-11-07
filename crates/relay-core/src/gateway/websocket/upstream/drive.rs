use std::time::Instant;

use super::super::*;
use super::handshake::{
    AuthorizationUpgrade, can_use_optional_fast_path, upgrade_with_authorization_refresh,
};
use super::open::{OpenedSocket, open_upgraded_socket};
use super::telemetry::record_connect_failure;
use super::upgrade_reject::{UpgradeAction, handle_upgrade_rejection};
use super::{ConnectRecovery, ConnectScope, ConnectTrace, Connected, HttpFallback};
use crate::runtime::{AccountTransport, WebsocketTransportPreference};

#[allow(clippy::large_enum_variant)]
pub(super) enum DrivenConnect {
    Continue(ClientRequest),
    Break(ClientRequest),
    Ready(Connected),
    Http(Box<HttpFallback>),
}

pub(super) struct DriveCandidateInput<'a> {
    pub(super) selected: crate::Selection,
    pub(super) route: ExecutorRoute,
    pub(super) lease: CandidateLease,
    pub(super) request: ClientRequest,
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) client_headers: &'a HeaderMap,
    pub(super) attempt: &'a mut u16,
    pub(super) recovery: &'a mut ConnectRecovery,
}

/// Drive one reserved WebSocket candidate through upgrade. Continue and break
/// hand the same request back; a ready socket consumes it.
pub(super) async fn drive_selected_candidate(
    connect_input: DriveCandidateInput<'_>,
) -> Result<DrivenConnect, GatewayFailure> {
    let DriveCandidateInput {
        selected,
        mut route,
        lease,
        mut request,
        runtime,
        key,
        client_headers,
        attempt,
        recovery,
    } = connect_input;
    recovery.tried.insert(selected.candidate_id.clone());
    let response_affinity_hit = selected.response_affinity_hit;
    runtime.set_route_streaming(&mut route, false);
    route.client_transport = crate::UsageTransport::Websocket;
    request.apply_service_tier();
    route.service_tier = request.service_tier();
    route.half_open_probe = selected.half_open_probe;
    route.client_context_id = request.client_context_id.clone();
    let source_error_origin = route_error_origin(&route);
    if route.client_wire_api != WireApi::Responses {
        return Ok(DrivenConnect::Continue(request));
    }
    // These account transports use HTTP/SSE. Keep their admission when handing
    // off to the HTTP executor: reservation already advanced the pool cursor.
    let basis_points = route.account_transport == AccountTransport::ExcelBasisPoints;
    let siwc = route.account_transport == AccountTransport::Siwc;
    if basis_points || siwc {
        return Ok(DrivenConnect::Http(Box::new(HttpFallback::for_candidate(
            request,
            key,
            selected,
            lease,
            crate::usage::TransportFallbackReason::WebsocketUnavailable,
            source_error_origin,
        ))));
    }
    route.routing = Some(selected.diagnostics.clone());
    if !route.adapter.is_passthrough() {
        drop(lease);
        recovery.http_fallback_origin = Some(source_error_origin);
        recovery.last_failure = Some(GatewayFailure::websocket_http_fallback(source_error_origin));
        return Ok(DrivenConnect::Continue(request));
    }
    let started = Instant::now();
    // Reads the current attempt. The dispatch number changes after the request is sent.
    macro_rules! connect_trace {
        () => {
            &ConnectTrace {
                event: None,
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
    let prepared = match runtime.prepare_route_authorization(&route, now_ms()).await {
        Ok(prepared) => prepared,
        Err(error) => {
            let failure = GatewayFailure::prepare(error, source_error_origin);
            record_connect_failure(connect_trace!(), &failure, None);
            recovery.last_failure = Some(failure);
            return Ok(DrivenConnect::Continue(request));
        }
    };
    if request.websocket_reauth(&route.candidate_id, &prepared)
        && request.has_previous_response_id()
        && !request.drop_previous_response_id(runtime, &key.id)
    {
        let failure = GatewayFailure::continuation_unavailable();
        record_connect_failure(connect_trace!(), &failure, None);
        return Err(failure);
    }
    let capability_scope = GatewayRuntime::websocket_capability_scope(&route, &prepared);
    let transport_preference = runtime.websocket_transport_preference(
        &capability_scope,
        &request.resolved_model,
        &prepared.incarnation(),
        now_ms(),
    );
    if transport_preference == WebsocketTransportPreference::SlowHandshake
        && can_use_optional_fast_path(&route, &request)
    {
        return Ok(DrivenConnect::Http(Box::new(HttpFallback::for_candidate(
            request,
            key,
            selected,
            lease,
            crate::usage::TransportFallbackReason::WebsocketSlowHandshake,
            source_error_origin,
        ))));
    }
    if transport_preference == WebsocketTransportPreference::HttpOnly {
        drop(lease);
        recovery.http_fallback_origin = Some(source_error_origin);
        recovery.last_failure = Some(GatewayFailure::websocket_http_fallback(source_error_origin));
        return Ok(DrivenConnect::Continue(request));
    }
    let mut prepared = prepared;
    let upgrade = match upgrade_with_authorization_refresh(
        runtime,
        client_headers,
        key,
        &mut route,
        &request,
        &lease,
        *attempt,
        started,
        source_error_origin,
        &mut prepared,
        &mut recovery.last_failure,
    )
    .await
    {
        AuthorizationUpgrade::ContinueCandidate => return Ok(DrivenConnect::Continue(request)),
        AuthorizationUpgrade::BudgetExhausted => return Ok(DrivenConnect::Break(request)),
        AuthorizationUpgrade::FastPathMissed => {
            return Ok(DrivenConnect::Http(Box::new(HttpFallback::for_candidate(
                request,
                key,
                selected,
                lease,
                crate::usage::TransportFallbackReason::WebsocketSlowHandshake,
                source_error_origin,
            ))));
        }
        AuthorizationUpgrade::Upgraded(upgrade) => upgrade,
    };
    // The final prepared authorization may differ from the first attempt
    // after an in-band 401 refresh. Preserve its generation on this
    // request-local route so every later WebSocket usage event is tied to
    // the credential that actually performed the upgrade.
    route.account_token_generation = prepared.token_generation;
    route.account_token_revision = prepared.token_revision.clone();
    let status = upgrade.status();
    let response_headers = upgrade.headers().clone();
    runtime.observe_codex_quota_headers(
        &route.candidate_id,
        prepared.token_generation,
        prepared.token_revision.as_ref(),
        status,
        &response_headers,
        now_ms(),
    );
    if status == StatusCode::SWITCHING_PROTOCOLS {
        runtime.observe_codex_models_etag(
            &route.candidate_id,
            &route.upstream_url,
            &response_headers,
            &prepared,
        );
    }
    if status != StatusCode::SWITCHING_PROTOCOLS {
        let mut scope = ConnectScope {
            event: None,
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
        match handle_upgrade_rejection(
            upgrade,
            status,
            &response_headers,
            &prepared,
            &mut scope,
            &mut recovery.progress(),
        )
        .await?
        {
            UpgradeAction::ContinueCandidates => return Ok(DrivenConnect::Continue(request)),
            UpgradeAction::Break => return Ok(DrivenConnect::Break(request)),
            UpgradeAction::Fail(failure) => return Err(failure),
        }
    }
    // A 401 during upgrade may have replaced the credential. Scope the body
    // against the final authorization immediately before sending response.create.
    let request_payload =
        request.observed_payload_for(runtime, &mut route, client_headers, &prepared)?;
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
        &mut recovery.last_failure,
    )
    .await?
    {
        OpenedSocket::Continue(returned) => Ok(DrivenConnect::Continue(returned)),
        OpenedSocket::Ready(connected) => Ok(DrivenConnect::Ready(connected)),
    }
}

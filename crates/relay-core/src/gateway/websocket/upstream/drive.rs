use std::collections::HashSet;
use std::time::Instant;

use super::super::super::execution::AttemptRepairs;
use super::super::*;
use super::handshake::{upgrade_with_authorization_refresh, AuthorizationUpgrade};
use super::open::{open_upgraded_socket, OpenedSocket};
use super::telemetry::record_connect_failure;
use super::upgrade_reject::{handle_upgrade_rejection, UpgradeAction};
use super::{ConnectProgress, ConnectScope, ConnectTrace, Connected, HttpFallback};
use crate::runtime::AccountTransport;

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
        mut route,
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
        let reserved_candidate = super::super::super::execution::ReservedCandidate {
            local_key_id: key.id.clone(),
            model: request.resolved_model.clone(),
            selection: selected,
            lease,
        };
        return Ok(DrivenConnect::Http(Box::new(HttpFallback {
            request,
            reserved_candidate,
        })));
    }
    route.routing = Some(selected.diagnostics);
    if !route.adapter.is_passthrough() {
        drop(lease);
        *websocket_http_fallback_origin = Some(source_error_origin);
        *last_failure = Some(GatewayFailure::websocket_http_fallback(source_error_origin));
        return Ok(DrivenConnect::Continue(request));
    }
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
    let prepared = match runtime.prepare_route_authorization(&route, now_ms()).await {
        Ok(prepared) => prepared,
        Err(error) => {
            let failure = GatewayFailure::prepare(error, source_error_origin);
            record_connect_failure(connect_trace!(), &failure, None);
            *last_failure = Some(failure);
            return Ok(DrivenConnect::Continue(request));
        }
    };
    let capability_scope = GatewayRuntime::websocket_capability_scope(&route, &prepared);
    if runtime.websocket_is_http_only(
        &capability_scope,
        &request.resolved_model,
        &prepared.incarnation(),
        now_ms(),
    ) {
        drop(lease);
        *websocket_http_fallback_origin = Some(source_error_origin);
        *last_failure = Some(GatewayFailure::websocket_http_fallback(source_error_origin));
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
        last_failure,
    )
    .await
    {
        AuthorizationUpgrade::ContinueCandidate => return Ok(DrivenConnect::Continue(request)),
        AuthorizationUpgrade::BudgetExhausted => return Ok(DrivenConnect::Break(request)),
        AuthorizationUpgrade::FastPathMissed => {
            return Err(GatewayFailure::websocket_slow_handshake(
                source_error_origin,
            ));
        }
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
        runtime.observe_codex_models_etag(
            &route.candidate_id,
            &route.upstream_url,
            &response_headers,
            &prepared,
        );
        let turn_scope = request_scope(
            &key.id,
            client_headers,
            route.account_id.as_deref(),
            &route.source_model,
            route.upstream_url.as_str(),
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
            &prepared,
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

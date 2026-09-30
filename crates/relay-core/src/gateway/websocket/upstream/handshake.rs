use std::time::Instant;

use reqwest_websocket::UpgradeResponse;

use super::super::*;
use super::connect::upstream_headers;
use super::telemetry::record_connect_failure;
use super::ConnectTrace;
use crate::runtime::PreparedAuthorization;

#[allow(clippy::large_enum_variant)]
pub(super) enum AuthorizationUpgrade {
    ContinueCandidate,
    Upgraded(UpgradeResponse),
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn upgrade_with_authorization_refresh(
    runtime: &GatewayRuntime,
    client_headers: &HeaderMap,
    key: &AuthenticatedKey,
    route: &ExecutorRoute,
    request: &ClientRequest,
    lease: &CandidateLease,
    attempt: u16,
    started: Instant,
    source_error_origin: ErrorOrigin,
    prepared: &mut PreparedAuthorization,
    last_failure: &mut Option<GatewayFailure>,
) -> AuthorizationUpgrade {
    let mut refresh_fence = None;
    loop {
        let mut headers = upstream_headers(
            client_headers,
            prepared,
            route.account_id.is_some() && request.responses_lite_for(route),
            &request.request_id,
        );
        apply_codex_routing_hint(&mut headers, &route.source_model, route.service_tier);
        let turn_scope = request_scope(
            &key.id,
            client_headers,
            route.account_id.as_deref(),
            &route.source_model,
        );
        runtime.guard_turn_state(&mut headers, turn_scope.as_ref(), prepared);
        let cookies = runtime.routing_cookies(&route.candidate_id, prepared);
        if let Some(cookies) = &cookies {
            cookies.apply(&route.upstream_url, &mut headers);
        }
        let upgrade = runtime
            .websocket_client(&route.candidate_id)
            .get(route.upstream_url.clone())
            .headers(headers)
            .upgrade();
        let Ok(Ok(upgrade)) = timeout(UPSTREAM_CONNECT_TIMEOUT, upgrade.send()).await else {
            return continue_candidate(
                runtime,
                lease,
                key,
                route,
                request,
                attempt,
                started,
                GatewayFailure::transport(source_error_origin),
                last_failure,
            );
        };
        if let Some(cookies) = cookies {
            cookies.observe(&route.upstream_url, upgrade.headers());
        }
        if upgrade.status() != StatusCode::UNAUTHORIZED
            || prepared.token_generation.is_none()
            || refresh_fence.is_some()
        {
            return AuthorizationUpgrade::Upgraded(upgrade);
        }
        drop(upgrade);
        refresh_fence = runtime.fence_execution(&route.candidate_id);
        match runtime
            .refresh_authorization_after_unauthorized(
                &route.candidate_id,
                prepared.token_generation,
                now_ms(),
            )
            .await
        {
            Ok(refreshed) => *prepared = refreshed,
            Err(error) => {
                return continue_candidate(
                    runtime,
                    lease,
                    key,
                    route,
                    request,
                    attempt,
                    started,
                    GatewayFailure::prepare(error, source_error_origin),
                    last_failure,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn continue_candidate(
    runtime: &GatewayRuntime,
    lease: &CandidateLease,
    key: &AuthenticatedKey,
    route: &ExecutorRoute,
    request: &ClientRequest,
    attempt: u16,
    started: Instant,
    failure: GatewayFailure,
    last_failure: &mut Option<GatewayFailure>,
) -> AuthorizationUpgrade {
    record_connect_failure(
        &ConnectTrace {
            runtime,
            lease,
            key,
            route,
            request,
            attempt,
            started,
        },
        &failure,
        None,
    );
    *last_failure = Some(failure);
    AuthorizationUpgrade::ContinueCandidate
}

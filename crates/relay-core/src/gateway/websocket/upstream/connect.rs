use std::collections::HashSet;

use super::super::super::execution::AttemptRepairs;
use super::super::*;
use super::drive::{drive_selected_candidate, DriveCandidateInput, DrivenConnect};
use super::selection_gap::{recover_without_candidate, GapAction, SelectionGap};
use super::Connected;

pub(super) async fn connect_upstream(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    client_headers: &HeaderMap,
    mut request: ClientRequest,
    allow_previous_response_reset: bool,
    attempt_offset: u16,
) -> Result<Connected, GatewayFailure> {
    let mut tried = HashSet::new();
    let mut attempt = attempt_offset;
    let mut confirmed_response_missing = false;
    let mut repairs = AttemptRepairs::default();
    let mut last_failure: Option<GatewayFailure> = None;
    let mut websocket_http_fallback_origin = None;
    let mut retry_window_expired = false;

    loop {
        request.account_retained_input();
        request.budget.configure_retry_window(
            runtime.route_recovery_window_ms(),
            runtime.route_recovery_enabled(),
        );
        if !request.budget.can_dispatch() {
            break;
        }
        // Read the live setting on every retry cycle so disabling it wakes
        // an active persistent wait through the configuration event.
        let wait_for_candidate_availability = runtime.route_recovery_enabled();
        if !repairs.quota_yield {
            repairs.quota_yield = true;
            if let Some(affinity_key) = request.response_affinity_key.clone() {
                if runtime.automatic_response_owner_should_yield_for_quota(
                    key,
                    &affinity_key,
                    &request.resolved_model,
                    WEBSOCKET_PROTOCOLS,
                    &tried,
                    now_ms(),
                ) {
                    let _ = request.drop_previous_response_id(runtime, &key.id);
                }
            }
        }
        let selected = runtime
            .select_and_reserve_with_budget(
                key,
                &request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                &tried,
                (
                    request.response_affinity_key.as_deref(),
                    request.prompt_affinity_key.as_deref(),
                ),
                now_ms(),
                &request.budget,
            )
            .await;
        let Some((selected, lease)) = selected else {
            let mut gap = SelectionGap {
                runtime,
                key,
                request: &mut request,
                tried: &mut tried,
                last_failure: &last_failure,
                http_fallback_origin: &websocket_http_fallback_origin,
                allow_previous_response_reset,
                repairs: &mut repairs,
                retry_window_expired: &mut retry_window_expired,
                wait_for_candidate_availability,
            };
            match recover_without_candidate(&mut gap).await {
                GapAction::Continue => continue,
                GapAction::Break => break,
                GapAction::Fail(failure) => return Err(failure),
            }
        };
        match drive_selected_candidate(DriveCandidateInput {
            selected,
            lease,
            request,
            runtime,
            key,
            client_headers,
            attempt: &mut attempt,
            tried: &mut tried,
            repairs: &mut repairs,
            confirmed_response_missing: &mut confirmed_response_missing,
            last_failure: &mut last_failure,
            http_fallback_origin: &mut websocket_http_fallback_origin,
        })
        .await?
        {
            DrivenConnect::Continue(returned) => {
                request = returned;
            }
            DrivenConnect::Break(returned) => {
                request = returned;
                break;
            }
            DrivenConnect::Ready(connected) => return Ok(connected),
        }
    }

    if allow_previous_response_reset
        && request.has_previous_response_id()
        && confirmed_response_missing
    {
        let mut reset_request = request.clone();
        if reset_request.drop_previous_response_id(runtime, &key.id) {
            return Box::pin(connect_upstream(
                runtime,
                key,
                client_headers,
                reset_request,
                false,
                attempt,
            ))
            .await;
        }
        return Err(GatewayFailure::continuation_unavailable());
    }

    if let Some(reason) = request.budget.admission_stop_reason() {
        return Err(GatewayFailure::admission(reason));
    }
    if retry_window_expired {
        return Err(GatewayFailure::classified(
            StatusCode::SERVICE_UNAVAILABLE,
            error_codes::UPSTREAM_UNAVAILABLE,
            ErrorOrigin::Relay,
        ));
    }
    if let Some(origin) = websocket_http_fallback_origin {
        return Err(GatewayFailure::websocket_http_fallback(origin));
    }
    if let Some(retry_at_ms) = runtime.earliest_retry_at(
        key,
        &request.resolved_model,
        WEBSOCKET_PROTOCOLS,
        &HashSet::new(),
        request.response_affinity_key.as_deref(),
        now_ms(),
        crate::scheduler::rotation::RotationOperation::Text,
    ) {
        return Err(GatewayFailure::cooldown(retry_at_ms));
    }
    Err(last_failure.unwrap_or_else(GatewayFailure::unavailable))
}

pub(super) fn upstream_headers(
    client_headers: &HeaderMap,
    prepared: &crate::runtime::PreparedAuthorization,
    responses_lite: bool,
    request_id: &str,
) -> HeaderMap {
    let mut headers = forwarded_codex_headers(client_headers, request_id);
    headers.insert(AUTHORIZATION, prepared.authorization.clone());
    if let Some(identity) = prepared.identity.as_ref() {
        let identity = codex_client_version(client_headers)
            .and_then(|version| identity.with_client_version(version).ok())
            .unwrap_or_else(|| identity.clone());
        identity.insert(&mut headers);
    }
    if responses_lite {
        headers.insert(
            HeaderName::from_static(CODEX_RESPONSES_LITE_HEADER),
            HeaderValue::from_static("true"),
        );
    }
    ensure_websocket_beta(&mut headers);
    headers
}

fn ensure_websocket_beta(headers: &mut HeaderMap) {
    let name = HeaderName::from_static("openai-beta");
    let present = headers
        .get_all(&name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.contains("responses_websockets="));
    if !present {
        headers.append(name, HeaderValue::from_static(RESPONSES_WEBSOCKET_BETA));
    }
}

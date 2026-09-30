use super::super::fallback::websocket_transport_fallback_status;
use super::super::*;
use super::telemetry::{
    record_connect_affinity_miss, record_connect_failure_with_hint, record_connect_rejection,
};
use super::{ConnectProgress, ConnectScope};

pub(super) enum UpgradeAction {
    ContinueCandidates,
    Break,
    Fail(GatewayFailure),
}

pub(super) async fn handle_upgrade_rejection(
    upgrade: reqwest_websocket::UpgradeResponse,
    status: StatusCode,
    response_headers: &HeaderMap,
    scope: &mut ConnectScope<'_>,
    progress: &mut ConnectProgress<'_>,
) -> Result<UpgradeAction, GatewayFailure> {
    let runtime = scope.runtime;
    let key = scope.key;
    let route = scope.route;
    let lease = scope.lease;
    let source_error_origin = scope.source_error_origin;
    let response_affinity_hit = scope.response_affinity_hit;
    let tried = &mut *progress.tried;
    let repairs = &mut *progress.repairs;
    let confirmed_response_missing = &mut *progress.confirmed_response_missing;
    let last_failure = &mut *progress.last_failure;
    let websocket_http_fallback_origin = &mut *progress.http_fallback_origin;
    let legacy_call_id_repair_attempted = &mut repairs.legacy_call_id;
    let native_replay_attempted = &mut repairs.native_replay;
    let stale_tool_history_recovered = &mut repairs.stale_tool_history;
    let model_switch_reset_attempted = &mut repairs.model_switch_reset;
    let response = upgrade.into_inner();
    let body = timeout(
        UPSTREAM_CONNECT_TIMEOUT,
        crate::transport::collect_limited(response, MAX_WEBSOCKET_ERROR_BYTES),
    )
    .await
    .ok()
    .and_then(Result::ok);
    let failure = GatewayFailure::upstream_status(status, body.as_deref(), source_error_origin);
    if !*legacy_call_id_repair_attempted
        && body
            .as_deref()
            .is_some_and(super::super::super::errors::responses_tool_call_links_rejected)
        && scope.request.repair_legacy_call_ids()
    {
        *legacy_call_id_repair_attempted = true;
        tried.remove(&route.candidate_id);
        lease.allow_rotation_repair();
        return Ok(UpgradeAction::ContinueCandidates);
    }
    if websocket_transport_fallback_status(status) {
        runtime.mark_websocket_http_only(
            &route.candidate_id,
            &scope.request.resolved_model,
            now_ms(),
        );
        *websocket_http_fallback_origin = Some(source_error_origin);
        *last_failure = Some(GatewayFailure::websocket_http_fallback(source_error_origin));
        return Ok(UpgradeAction::ContinueCandidates);
    }
    let response_missing = body
        .as_deref()
        .is_some_and(super::super::super::errors::previous_response_not_found);
    let affinity_miss = super::super::super::errors::recoverable_response_affinity_miss(
        status,
        scope.request.has_previous_response_id(),
        response_affinity_hit,
        response_missing,
    );
    let model_switch_reset = !*model_switch_reset_attempted
        && super::super::super::errors::recoverable_response_model_switch(
            status,
            failure.category,
            scope.request.has_previous_response_id(),
            scope.request.has_unpaired_tool_output(),
            body.as_deref().unwrap_or_default(),
        );
    if affinity_miss
        && response_affinity_hit
        && scope.request.replay_missing_response(
            runtime,
            &key.id,
            route,
            &mut *native_replay_attempted,
        )?
    {
        record_connect_affinity_miss(&scope.trace(), status);
        tried.remove(&route.candidate_id);
        lease.allow_rotation_repair();
        *last_failure = Some(failure);
        return Ok(UpgradeAction::ContinueCandidates);
    }
    let stale_tool_history = !*stale_tool_history_recovered
        && scope.request.has_previous_response_id()
        && body.as_deref().is_some_and(|body| {
            super::super::super::errors::responses_tool_call_is_missing_output(body)
                && scope
                    .request
                    .recover_stale_tool_history(runtime, &key.id, body)
        });
    if stale_tool_history {
        *stale_tool_history_recovered = true;
        record_connect_rejection(&scope.trace(), &failure);
        *last_failure = Some(failure);
        return Ok(UpgradeAction::ContinueCandidates);
    }
    if model_switch_reset && scope.request.drop_previous_response_id(runtime, &key.id) {
        *model_switch_reset_attempted = true;
        *last_failure = Some(failure);
        return Ok(UpgradeAction::ContinueCandidates);
    }
    if body
        .as_deref()
        .is_some_and(super::super::super::errors::prompt_cache_write_rejected)
    {
        runtime.invalidate_prompt_affinity(scope.request.prompt_affinity_key.as_deref());
        record_connect_failure_with_hint(
            &scope.trace(),
            &failure,
            Some(response_headers),
            body.as_deref()
                .map(rate_limit_body_hint)
                .unwrap_or_default(),
        );
        *last_failure = Some(failure);
        return Ok(UpgradeAction::ContinueCandidates);
    }
    if affinity_miss {
        *confirmed_response_missing |= response_missing;
        runtime.invalidate_response_affinity(scope.request.response_affinity_key.as_deref());
        record_connect_affinity_miss(&scope.trace(), status);
        *last_failure = Some(failure);
        if response_missing && response_affinity_hit {
            return Ok(UpgradeAction::Break);
        }
        return Ok(UpgradeAction::ContinueCandidates);
    }
    if super::super::super::errors::retryable_failure(
        status,
        failure.category,
        scope.request.has_previous_response_id(),
    ) {
        if response_affinity_hit && !scope.request.requires_affinity_owner {
            scope.request.response_affinity_key = None;
        }
        record_connect_failure_with_hint(
            &scope.trace(),
            &failure,
            Some(response_headers),
            body.as_deref()
                .map(rate_limit_body_hint)
                .unwrap_or_default(),
        );
        *last_failure = Some(failure);
        return Ok(UpgradeAction::ContinueCandidates);
    }
    record_connect_rejection(&scope.trace(), &failure);
    Ok(UpgradeAction::Fail(failure))
}

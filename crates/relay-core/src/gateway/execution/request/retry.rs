use super::prelude::*;
use super::recovery::adapter_error_response_for_origin;

pub(in crate::gateway::execution) fn mark_adapter_failure(
    mut event: UsageEvent,
    error: &AdapterError,
) -> UsageEvent {
    event.success = false;
    event.http_status = StatusCode::BAD_GATEWAY.as_u16();
    event.error_category = Some(error.code().to_string());
    event
}

pub(in crate::gateway::execution) struct BasisPointsRelayRetryContext<'a> {
    pub(in crate::gateway::execution) attempted: &'a mut bool,
    pub(in crate::gateway::execution) parameter: &'a mut Option<&'static str>,
    pub(in crate::gateway::execution) runtime: &'a GatewayRuntime,
    pub(in crate::gateway::execution) tried: &'a mut HashSet<String>,
    pub(in crate::gateway::execution) candidate_id: &'a str,
    pub(in crate::gateway::execution) lease: &'a CandidateLease,
    pub(in crate::gateway::execution) last_adapter_error: &'a mut Option<AdapterError>,
}

/// Apply the bounded Basis Points regeneration path shared by pooled and
/// account-only execution. Returning the original error and usage event keeps
/// the caller's terminal-error response path intact when the retry is not
/// eligible.
pub(in crate::gateway::execution) fn handle_basis_points_relay_retry(
    error: AdapterError,
    body: &[u8],
    event: UsageEvent,
    context: BasisPointsRelayRetryContext<'_>,
) -> Result<(), Box<(AdapterError, UsageEvent)>> {
    let BasisPointsRelayRetryContext {
        attempted,
        parameter,
        runtime,
        tried,
        candidate_id,
        lease,
        last_adapter_error,
    } = context;
    if !super::super::basis_points::take_tool_relay_retry(error, body, attempted, parameter) {
        return Err(Box::new((error, event)));
    }

    emit_usage(runtime, mark_adapter_failure(event, &error));
    *last_adapter_error = Some(error);
    tried.remove(candidate_id);
    lease.allow_rotation_repair();
    lease.settle_rotation_repair(now_ms());
    Ok(())
}

pub(in crate::gateway::execution) fn basis_points_relay_error_response(
    error: AdapterError,
    event: UsageEvent,
    runtime: &GatewayRuntime,
    lease: &CandidateLease,
    origin: ErrorOrigin,
) -> Response<Body> {
    emit_usage(runtime, mark_adapter_failure(event, &error));
    lease.settle_rotation_terminal(now_ms());
    adapter_error_response_for_origin(error, origin)
}

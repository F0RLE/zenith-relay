use super::prelude::*;

pub(in crate::gateway::execution) fn should_wait_for_candidate_availability(
    enabled: bool,
    last_failure: &Option<AttemptFailure>,
    has_adapter_error: bool,
    has_previous_response_id: bool,
) -> bool {
    enabled
        && !has_adapter_error
        && last_failure.as_ref().is_none_or(|failure| {
            crate::gateway::errors::retryable_recovery_wait(
                failure.status,
                failure.category,
                has_previous_response_id,
            )
        })
}

pub(in crate::gateway::execution) fn recover_stale_tool_history(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    request: &mut Value,
    model: &str,
    upstream_error: &[u8],
    recovered: &mut bool,
) -> bool {
    let stream = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if *recovered
        || !replay_and_prune_stale_tool_history(
            runtime,
            local_key_id,
            request,
            model,
            now_ms(),
            stream,
            upstream_error,
        )
    {
        return false;
    }
    *recovered = true;
    true
}

pub(super) fn request_has_previous_response_id(client_wire_api: WireApi, request: &Value) -> bool {
    client_wire_api == WireApi::Responses && previous_response_id(request).is_some()
}

/// Applies the one permitted repair for a strict upstream rejection, then
/// recomputes route-affinity state from the repaired request before retrying.
/// The same rejection can arrive as either a buffered error or a terminal
/// stream bootstrap failure, so both paths share this mutation.
pub(super) struct LegacyCallIdRepair<'a> {
    pub(super) request: &'a mut Value,
    pub(super) client_wire_api: WireApi,
    pub(super) adapter_is_passthrough: bool,
    pub(super) upstream_rejected_tool_links: bool,
    pub(super) repair_attempted: &'a mut bool,
    pub(super) tried: &'a mut HashSet<String>,
    pub(super) candidate_id: &'a str,
    pub(super) has_unpaired_tool_output: &'a mut bool,
    pub(super) requires_affinity_owner: &'a mut bool,
}

pub(super) fn try_repair_legacy_responses_call_ids(repair: LegacyCallIdRepair<'_>) -> bool {
    let LegacyCallIdRepair {
        request,
        client_wire_api,
        adapter_is_passthrough,
        upstream_rejected_tool_links,
        repair_attempted,
        tried,
        candidate_id,
        has_unpaired_tool_output,
        requires_affinity_owner,
    } = repair;
    if client_wire_api != WireApi::Responses
        || !adapter_is_passthrough
        || *repair_attempted
        || !upstream_rejected_tool_links
        || !repair_legacy_responses_call_ids(request)
    {
        return false;
    }

    *repair_attempted = true;
    tried.remove(candidate_id);
    *has_unpaired_tool_output = !unpaired_tool_output_ids(request).is_empty();
    *requires_affinity_owner =
        request_has_previous_response_id(client_wire_api, request) || *has_unpaired_tool_output;
    true
}

pub(super) fn replay_native_tool_continuation(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    request: &mut Value,
    route: &crate::runtime::ExecutorRoute,
    stream: bool,
    attempted: &mut bool,
) -> Result<bool, AdapterError> {
    let Some(previous_response_id) = previous_response_id(request) else {
        return Ok(false);
    };
    let Some(replay) = runtime.load_native_responses_replay(
        local_key_id,
        previous_response_id,
        &route.candidate_id,
        now_ms(),
    ) else {
        return Ok(false);
    };
    *request = replay.replay_request(request, &route.source_model, stream)?;
    *attempted = true;
    Ok(true)
}

pub(super) fn replay_native_affinity_continuation(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    request: &mut Value,
    response_affinity_key: Option<&str>,
    model: &str,
    stream: bool,
    attempted: &mut bool,
) -> Result<bool, AdapterError> {
    let Some(previous_response_id) = previous_response_id(request) else {
        return Ok(false);
    };
    let Some(candidate_id) =
        response_affinity_key.and_then(|key| runtime.response_affinity_candidate(key, now_ms()))
    else {
        return Ok(false);
    };
    let Some(replay) = runtime.load_native_responses_replay(
        local_key_id,
        previous_response_id,
        &candidate_id,
        now_ms(),
    ) else {
        return Ok(false);
    };
    *request = match replay.replay_request(request, model, stream) {
        Ok(request) => request,
        Err(error) if error.code() == error_codes::ADAPTER_CONTINUATION_MISMATCH => {
            return Ok(false)
        }
        Err(error) => return Err(error),
    };
    *attempted = true;
    Ok(true)
}

pub(in crate::gateway::execution) fn adapter_error_response(error: AdapterError) -> Response<Body> {
    adapter_error_response_for_origin(error, crate::ErrorOrigin::Relay)
}

pub(in crate::gateway::execution) fn adapter_error_response_for_origin(
    error: AdapterError,
    origin: crate::ErrorOrigin,
) -> Response<Body> {
    let status = if error.is_upstream_failure() {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::BAD_REQUEST
    };
    let message = error
        .parameter()
        .map(|parameter| format!("{} (parameter: {parameter})", error.message()));
    super::super::super::errors::api_error_with_parameter(
        status,
        message.as_deref().unwrap_or(error.message()),
        error.code(),
        error.code(),
        origin,
        None,
        error.parameter(),
    )
}

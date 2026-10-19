use crate::{GatewayRuntime, WireApi};
use serde_json::Value;

fn reference(local_key: &str, request: &Value, token: &str) -> Result<String, ()> {
    if token.is_empty() || !request.get("contents").is_some_and(Value::is_array) {
        return Err(());
    }
    // The token stays opaque; bind its owner to the caller, model and exact original contents.
    let digest = crate::json_fingerprint::sha256(&(
        "gemini-continuation",
        local_key,
        request.get("model"),
        request.get("contents"),
        token,
    ))
    .map_err(|_| ())?;
    Ok(format!("gemini-continuation:{}", hex::encode(digest)))
}

pub(in crate::gateway) fn request_reference(
    local_key: &str,
    request: &Value,
) -> Result<Option<String>, ()> {
    request
        .get("continuationToken")
        .filter(|token| !token.is_null())
        .map(|token| reference(local_key, request, token.as_str().ok_or(())?))
        .transpose()
}

pub(in crate::gateway) fn prepare(
    runtime: &GatewayRuntime,
    local_key: &str,
    request: &Value,
    now_ms: u64,
) -> Result<super::ContinuationState, ()> {
    let key = match request_reference(local_key, request)? {
        Some(reference) => {
            let key = runtime.response_affinity_key(Some(&reference)).ok_or(())?;
            if !runtime.has_response_affinity_binding(&key, now_ms) {
                return Err(());
            }
            Some(key)
        }
        None => None,
    };
    Ok(super::ContinuationState {
        requires_affinity_owner: key.is_some(),
        response_affinity_key: key,
        has_unpaired_tool_output: false,
    })
}

pub(in crate::gateway) fn bind_response(
    runtime: &GatewayRuntime,
    local_key: &str,
    route: &crate::runtime::ExecutorRoute,
    request: &Value,
    response: &Value,
    now_ms: u64,
) {
    if !route.adapter.is_passthrough()
        || route.client_wire_api != WireApi::Gemini
        || crate::protocol::completion::ResponseCompletion::for_protocol(WireApi::Gemini, response)
            != crate::protocol::completion::ResponseCompletion::Incomplete
    {
        return;
    }
    for candidate in response
        .get("candidates")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if candidate["finishReason"] == "CONTINUATION" {
            if let Some(token) = candidate.get("continuationToken").and_then(Value::as_str) {
                if let Ok(reference) = reference(local_key, request, token) {
                    runtime.bind_response_route_affinity(Some(&reference), route, now_ms);
                }
            }
        }
    }
}

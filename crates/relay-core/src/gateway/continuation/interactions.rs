//! Native Interactions IDs remain bound to their caller and exact source route.
use crate::{GatewayRuntime, WireApi};
use serde_json::Value;

use crate::protocol::interactions::reference;

pub(in crate::gateway) fn prepare(
    runtime: &GatewayRuntime,
    local_key: &str,
    request: &Value,
    now_ms: u64,
) -> Result<super::ContinuationState, ()> {
    let key = crate::protocol::interactions::previous_id(request)
        .map_err(|_| ())?
        .map(|id| {
            reference(local_key, id)
                .and_then(|reference| runtime.response_affinity_key(Some(&reference)).ok_or(()))
        })
        .transpose()?;
    if key
        .as_ref()
        .is_some_and(|key| !runtime.has_response_affinity_binding(key, now_ms))
    {
        return Err(());
    }
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
    response: &Value,
    now_ms: u64,
) {
    if route.client_wire_api != WireApi::GeminiInteractions
        || !route.adapter.is_passthrough()
        || crate::protocol::interactions::completion(response)
            != crate::protocol::completion::ResponseCompletion::Completed
    {
        return;
    }
    if let Some(id) = crate::protocol::interactions::resource(response)
        .get("id")
        .and_then(Value::as_str)
    {
        if let Ok(reference) = reference(local_key, id) {
            runtime.bind_response_route_affinity(Some(&reference), route, now_ms);
        }
    }
}

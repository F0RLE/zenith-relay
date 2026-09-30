use super::prelude::*;

/// Keeps adapter translation and JSON serialization at the protocol boundary.
/// Native responses preserve their upstream bytes, while bridged responses
/// return both the client representation and continuation state.
pub(super) fn translate_completed_response(
    adapter_request: PreparedAdapterRequest,
    upstream_bytes: Vec<u8>,
) -> Result<(Vec<u8>, Option<AdapterResponse>), AdapterError> {
    let bridge_response = adapter_request.translate_response_bytes(&upstream_bytes)?;
    let response_bytes = bridge_response
        .as_ref()
        .map(|response| {
            serde_json::to_vec(response.response_body())
                .map_err(|_| AdapterError::upstream_response_invalid())
        })
        .transpose()?
        .unwrap_or(upstream_bytes);
    Ok((response_bytes, bridge_response))
}

/// The Excel transport wraps tools inside Responses. Unwrap it before the
/// regular protocol adapter sees the response. For streamed client protocols,
/// feed the completed Responses response through that adapter's SSE bridge.
pub(super) struct CompletedBasisPointsResponse {
    pub(super) bytes: Vec<u8>,
    pub(super) bridge_response: Option<AdapterResponse>,
    pub(super) stream: Option<Vec<u8>>,
}

pub(super) fn translate_basis_points_completed(
    adapter_request: PreparedAdapterRequest,
    upstream_bytes: &[u8],
    responses_request: &Value,
    stream: bool,
) -> Result<CompletedBasisPointsResponse, AdapterError> {
    let responses_bytes =
        super::super::basis_points::translate_response(upstream_bytes, responses_request)?;
    if !stream {
        let (bytes, response) = translate_completed_response(adapter_request, responses_bytes)?;
        return Ok(CompletedBasisPointsResponse {
            bytes,
            bridge_response: response,
            stream: None,
        });
    }
    let responses_stream = super::super::basis_points::synthetic_stream(&responses_bytes)?;
    let Some(mut bridge) = adapter_request.into_stream_bridge() else {
        return Ok(CompletedBasisPointsResponse {
            bytes: responses_bytes,
            bridge_response: None,
            stream: Some(responses_stream),
        });
    };
    bridge.push(&responses_stream);
    bridge.finish();
    let completed = bridge
        .completed()
        .cloned()
        .ok_or_else(AdapterError::upstream_stream_invalid)?;
    let mut client_stream = Vec::new();
    while let Some(event) = bridge.pop_output() {
        client_stream.extend_from_slice(&event);
    }
    let bytes = serde_json::to_vec(&completed.response_body)
        .map_err(|_| AdapterError::upstream_response_invalid())?;
    Ok(CompletedBasisPointsResponse {
        bytes,
        bridge_response: Some(AdapterResponse::Translated(completed)),
        stream: Some(client_stream),
    })
}

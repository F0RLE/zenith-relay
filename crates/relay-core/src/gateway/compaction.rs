use super::errors::AttemptFailure;
use super::request::codex_client_version;
use super::streaming::{parse_sse_event, NativeReplayCapture, TerminalOutcome};
use crate::error_codes;
use crate::protocol::{ensure_compaction_trigger, sse_event_end};
use crate::runtime::{AuthorizationIdentityPolicy, CodexTurnStateScope, ExecutorRoute};
use crate::scheduler::rotation::SharedRequestBudget;
use crate::GatewayRuntime;
use axum::http::{
    header::{ACCEPT, CONTENT_TYPE},
    HeaderMap, StatusCode,
};
use serde_json::{json, Value};

pub(super) fn missing_legacy_endpoint(status: StatusCode, response_body: &[u8]) -> bool {
    if status == StatusCode::METHOD_NOT_ALLOWED {
        return true;
    }
    if status != StatusCode::NOT_FOUND {
        return false;
    }
    match serde_json::from_slice::<Value>(response_body) {
        Ok(response_json) => matches!(
            response_json
                .pointer("/error/code")
                .or_else(|| response_json.get("code"))
                .and_then(Value::as_str),
            Some("route_not_found" | "endpoint_not_found")
        ),
        Err(_) => {
            response_body.is_empty()
                || std::str::from_utf8(response_body)
                    .is_ok_and(|text| text.trim() == "404 page not found")
        }
    }
}

fn request_body(request_json: &Value) -> Result<Vec<u8>, AttemptFailure> {
    let mut request_json = request_json.clone();
    let input_items = request_json
        .get_mut("input")
        .and_then(Value::as_array_mut)
        .ok_or_else(AttemptFailure::invalid_request)?;
    ensure_compaction_trigger(input_items);
    let request_object = request_json
        .as_object_mut()
        .ok_or_else(AttemptFailure::invalid_request)?;
    request_object.insert("stream".into(), Value::Bool(true));
    request_object.insert("store".into(), Value::Bool(false));
    request_object.entry("tool_choice").or_insert(json!("auto"));
    request_object
        .entry("include")
        .or_insert(json!(["reasoning.encrypted_content"]));
    let bytes = serde_json::to_vec(&request_json).map_err(|_| AttemptFailure::invalid_request())?;
    Ok(bytes)
}

fn invalid_stream() -> AttemptFailure {
    AttemptFailure::classified_with_hint(
        StatusCode::BAD_GATEWAY,
        error_codes::COMPACTION_RESPONSE_INVALID,
        Default::default(),
    )
}

fn compact_output(mut bytes: &[u8]) -> Result<Vec<u8>, AttemptFailure> {
    let mut completed_response = None;
    let mut capture = NativeReplayCapture::default();
    while let Some(end) = sse_event_end(bytes) {
        let event = parse_sse_event(&bytes[..end]);
        bytes = &bytes[end..];
        if event.has_data && !event.valid {
            return Err(invalid_stream());
        }
        if let Some(event_payload) = &event.event_payload {
            if completed_response.is_some() {
                return Err(invalid_stream());
            }
            if event.outcome.is_none() {
                capture.observe(event_payload);
            }
        }
        if matches!(
            event.outcome,
            Some(TerminalOutcome::Failure | TerminalOutcome::Incomplete)
        ) {
            return Err(AttemptFailure::classified_with_hint(
                event.error_status.unwrap_or(StatusCode::BAD_GATEWAY),
                event
                    .error_category
                    .unwrap_or(error_codes::COMPACTION_RESPONSE_INVALID),
                event.cooldown_hint,
            ));
        }
        if let Some(event_response) = event
            .response
            .filter(|_| event.outcome == Some(TerminalOutcome::Success))
        {
            if completed_response.is_some() {
                return Err(invalid_stream());
            }
            completed_response = Some(event_response);
        }
    }
    if !bytes.trim_ascii().is_empty() {
        return Err(invalid_stream());
    }
    let mut compact_response = completed_response.ok_or_else(invalid_stream)?;
    if compact_response
        .get("status")
        .is_some_and(|status_value| status_value != "completed")
    {
        return Err(invalid_stream());
    }
    if compact_response
        .get("output")
        .is_none_or(|output_value| output_value.as_array().is_some_and(Vec::is_empty))
    {
        compact_response = capture
            .finish(Some(compact_response), None)
            .ok_or_else(invalid_stream)?;
    }
    compaction_document(&compact_response)
}

/// Codex compact response. Retained tail items stay in `output`; exactly one
/// encrypted compaction checkpoint is required.
pub(super) fn compaction_document(compaction_response: &Value) -> Result<Vec<u8>, AttemptFailure> {
    if compaction_response
        .get("status")
        .is_some_and(|status_value| status_value != "completed")
    {
        return Err(invalid_stream());
    }
    let response_items = compaction_response
        .get("output")
        .and_then(Value::as_array)
        .filter(|output_items| !output_items.is_empty())
        .ok_or_else(invalid_stream)?;
    let compaction_items = response_items
        .iter()
        .filter(|output_item| output_item.get("type").and_then(Value::as_str) == Some("compaction"))
        .collect::<Vec<_>>();
    if compaction_items.len() != 1
        || compaction_items[0]
            .get("encrypted_content")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(invalid_stream());
    }
    let mut compaction_json = json!({"object": "response.compaction", "output": response_items});
    for key in ["id", "created_at", "usage"] {
        if let Some(metadata_value) = compaction_response.get(key) {
            compaction_json[key] = metadata_value.clone();
        }
    }
    serde_json::to_vec(&compaction_json).map_err(|_| invalid_stream())
}

/// Makes a non-account `/v1/responses/compact` body an ordinary Responses
/// request. String and object input use the same array shape as account
/// compact, then Relay adds the trigger when the client omitted it.
pub(super) fn prepare_routed_compaction_request(
    request_object: &mut serde_json::Map<String, Value>,
) -> bool {
    if !super::request::coerce_responses_input_array(request_object) {
        return false;
    }
    let Some(input_items) = request_object
        .get_mut("input")
        .and_then(Value::as_array_mut)
    else {
        return false;
    };
    ensure_compaction_trigger(input_items);
    request_object.insert("stream".to_string(), Value::Bool(false));
    true
}

// Only an explicit missing legacy endpoint permits this single compatibility
// attempt. Its generation or malformed response must never be replayed.
pub(super) async fn execute(
    runtime: &GatewayRuntime,
    route: &mut ExecutorRoute,
    client_request: &Value,
    headers: &HeaderMap,
    scope: Option<&CodexTurnStateScope<'_>>,
    budget: &SharedRequestBudget,
    lease: &crate::runtime::CandidateLease,
) -> Result<(HeaderMap, Vec<u8>), Box<(AttemptFailure, HeaderMap)>> {
    let request_body =
        request_body(client_request).map_err(|failure| Box::new((failure, HeaderMap::new())))?;
    let upstream = runtime
        .send_authorized_request(
            &route.candidate_id,
            runtime
                .request_client(&route.candidate_id)
                .post(route.upstream_url.clone())
                .headers(headers.clone())
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "text/event-stream")
                .body(request_body),
            crate::runtime::AuthorizationDispatch {
                client_version: codex_client_version(headers),
                identity_policy: AuthorizationIdentityPolicy::RelayCodex,
                turn_scope: scope,
                budget: Some(budget),
                lease: Some(lease),
            },
        )
        .await
        .map_err(|error| Box::new((AttemptFailure::authorized_request(error), HeaderMap::new())))?;
    route.account_token_generation = upstream.account_token_generation;
    let upstream_response = upstream.response;
    let status = upstream_response.status();
    let mut headers = upstream_response.headers().clone();
    let response_bytes = crate::transport::collect(upstream_response)
        .await
        .map_err(|_| Box::new((invalid_stream(), headers.clone())))?;
    if !status.is_success() {
        let mut failure = AttemptFailure::status_with_body(status, Some(&response_bytes));
        super::errors::apply_degraded_route_policy(runtime, &mut failure);
        return Err(Box::new((failure, headers)));
    }
    let compacted_body =
        compact_output(&response_bytes).map_err(|failure| Box::new((failure, headers.clone())))?;
    for header_name in [
        "content-length",
        "content-encoding",
        "transfer-encoding",
        "trailer",
    ] {
        headers.remove(header_name);
    }
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
    Ok((headers, compacted_body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routed_compact_request_adds_a_trigger_without_requiring_an_account() {
        let mut request = serde_json::Map::new();
        request.insert("model".into(), json!("vendor-model"));
        request.insert("input".into(), json!("Keep src/main.rs"));
        request.insert(
            "tools".into(),
            json!([{"type": "custom", "name": "apply_patch"}]),
        );
        assert!(prepare_routed_compaction_request(&mut request));
        assert_eq!(request["stream"], false);
        assert_eq!(request["tools"][0]["name"], "apply_patch");
        let input = request["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "user");
        assert_eq!(input[1]["type"], "compaction_trigger");
        assert!(prepare_routed_compaction_request(&mut request));
        assert_eq!(request["input"].as_array().unwrap().len(), 2);

        let completed = json!({
            "id": "resp_compact",
            "status": "completed",
            "usage": {"input_tokens": 12, "output_tokens": 3},
            "output": [{"type": "compaction", "encrypted_content": "zenith-relay-compact-v1:test"}]
        });
        let encoded = compaction_document(&completed).ok().unwrap();
        let response_json: Value = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(response_json["object"], "response.compaction");
        assert_eq!(response_json["id"], "resp_compact");
        assert_eq!(response_json["usage"]["input_tokens"], 12);
        assert_eq!(response_json["output"][0]["type"], "compaction");
        assert!(compaction_document(&json!({"status": "incomplete", "output": []})).is_err());
    }

    #[test]
    fn bridge_preserves_history_and_request_fields() {
        let request = json!({"model":"future-model", "input":[{"type":"function_call_output","call_id":"call-1","output":"result"}], "tools":[{"type":"function","name":"test"}], "reasoning":{"effort":"high"}, "extension":true});
        let request_json: Value =
            serde_json::from_slice(&request_body(&request).ok().unwrap()).unwrap();
        assert_eq!(request_json["input"][0], request["input"][0]);
        assert_eq!(request_json["tools"], request["tools"]);
        assert_eq!(request_json["reasoning"], request["reasoning"]);
        assert_eq!(request_json["extension"], true);
        assert_eq!(request_json["input"][1]["type"], "compaction_trigger");
        let repeated_request: Value =
            serde_json::from_slice(&request_body(&request_json).ok().unwrap()).unwrap();
        assert_eq!(repeated_request, request_json);
    }

    #[test]
    fn mixed_stream_framing_preserves_compacted_history() {
        let output = json!([
            {"type":"compaction", "encrypted_content":"synthetic-checkpoint"},
            {"type":"message", "role":"user", "content":[{"type":"input_text","text":"Synthetic retained context"}]}
        ]);
        let terminal = json!({"type":"response.completed", "response":{
            "id":"synthetic-compaction", "status":"completed", "output":output,
            "usage":{"input_tokens":100,"output_tokens":8}
        }});
        let sse_body = format!(
            "data: {{\"type\":\"response.created\",\"response\":{{\"id\":\"synthetic-compaction\"}}}}\r\n\r\n: keep-alive\n\ndata: {terminal}\n\n"
        );
        let response: Value =
            serde_json::from_slice(&compact_output(sse_body.as_bytes()).ok().unwrap()).unwrap();
        assert_eq!(response["output"], output);
        assert_eq!(response["usage"]["input_tokens"], 100);
        assert_eq!(response["usage"]["output_tokens"], 8);
    }

    #[test]
    fn compaction_requires_terminal_success_and_real_encrypted_output() {
        let complete = b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"compaction\",\"encrypted_content\":\"synthetic\"}],\"usage\":{\"input_tokens\":42}}}\n\n";
        let response_json: Value =
            serde_json::from_slice(&compact_output(complete).ok().unwrap()).unwrap();
        assert_eq!(response_json["usage"]["input_tokens"], 42);
        assert!(response_json["usage"].get("output_tokens").is_none());
        assert!(compact_output(&complete[..complete.len() - 1]).is_err());
        assert!(compact_output(b"data: [DONE]\n\n").is_err());
        let failed = [
            complete.as_slice(),
            b"data: {\"type\":\"response.failed\"}\n\n",
        ]
        .concat();
        assert!(compact_output(&failed).is_err());
        assert!(compact_output(
            b"data: {\"type\":\"response.completed\",\"response\":{\"output\":[]}}\n\n"
        )
        .is_err());
    }

    #[test]
    fn sparse_completion_collects_ordered_items_and_rejects_unfinished_output() {
        let message = json!({"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":"synthetic summary"}]});
        let compaction = json!({"type":"compaction", "encrypted_content":"synthetic"});
        let frames = vec![
            json!({"type":"response.output_item.done", "output_index":1, "item":compaction}),
            json!({"type":"response.output_item.done", "output_index":0, "item":message}),
            json!({"type":"response.completed", "response":{"id":"synthetic-compact", "usage":{"input_tokens":12}}}),
        ];
        let sse = |frames: &[Value]| {
            frames
                .iter()
                .map(|frame| format!("data: {frame}\n\n"))
                .collect::<String>()
        };
        let complete = sse(&frames);
        let response_json: Value =
            serde_json::from_slice(&compact_output(complete.as_bytes()).ok().unwrap()).unwrap();
        assert_eq!(response_json["output"], json!([message, compaction]));
        assert_eq!(response_json["usage"], json!({"input_tokens":12}));
        assert!(compact_output(sse(&frames[..2]).as_bytes()).is_err());
        let unfinished = format!(
            "data: {}\n\n{complete}",
            json!({"type":"response.output_text.delta", "output_index":2, "delta":"unfinished"})
        );
        assert!(compact_output(unfinished.as_bytes()).is_err());
        let late = format!("{complete}data: {}\n\n", frames[0]);
        assert!(compact_output(late.as_bytes()).is_err());
    }

    #[test]
    fn model_errors_do_not_trigger_compaction_fallback() {
        assert!(!missing_legacy_endpoint(
            StatusCode::NOT_FOUND,
            br#"{"error":{"code":"model_not_found"}}"#
        ));
        assert!(!missing_legacy_endpoint(StatusCode::TOO_MANY_REQUESTS, b""));
        assert!(missing_legacy_endpoint(
            StatusCode::NOT_FOUND,
            b"404 page not found"
        ));
        assert!(missing_legacy_endpoint(StatusCode::METHOD_NOT_ALLOWED, b""));
    }
}

use super::errors::AttemptFailure;
use super::request::{codex_client_version, MAX_CLIENT_REQUEST_BODY_BYTES};
use super::streaming::{parse_sse_event, NativeReplayCapture, TerminalOutcome};
use crate::error_codes;
use crate::protocol::sse_event_end;
use crate::runtime::{CodexTurnStateScope, ExecutorRoute};
use crate::GatewayRuntime;
use axum::http::{
    header::{ACCEPT, CONTENT_TYPE},
    HeaderMap, StatusCode,
};
use serde_json::{json, Value};

pub(super) fn missing_legacy_endpoint(status: StatusCode, body: &[u8]) -> bool {
    if status == StatusCode::METHOD_NOT_ALLOWED {
        return true;
    }
    if status != StatusCode::NOT_FOUND {
        return false;
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(value) => matches!(
            value
                .pointer("/error/code")
                .or_else(|| value.get("code"))
                .and_then(Value::as_str),
            Some("route_not_found" | "endpoint_not_found")
        ),
        Err(_) => {
            body.is_empty()
                || std::str::from_utf8(body).is_ok_and(|text| text.trim() == "404 page not found")
        }
    }
}

fn request_body(request: &Value) -> Result<Vec<u8>, AttemptFailure> {
    let mut request = request.clone();
    let input = request
        .get_mut("input")
        .and_then(Value::as_array_mut)
        .ok_or_else(AttemptFailure::invalid_request)?;
    if !input
        .iter()
        .any(|item| item.get("type").and_then(Value::as_str) == Some("compaction_trigger"))
    {
        input.push(json!({"type": "compaction_trigger"}));
    }
    let object = request
        .as_object_mut()
        .ok_or_else(AttemptFailure::invalid_request)?;
    object.insert("stream".into(), Value::Bool(true));
    object.insert("store".into(), Value::Bool(false));
    object.entry("tool_choice").or_insert(json!("auto"));
    object
        .entry("include")
        .or_insert(json!(["reasoning.encrypted_content"]));
    let bytes = serde_json::to_vec(&request).map_err(|_| AttemptFailure::invalid_request())?;
    if bytes.len() > MAX_CLIENT_REQUEST_BODY_BYTES {
        return Err(AttemptFailure::invalid_request());
    }
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
    let mut completed = None;
    let mut capture = NativeReplayCapture::default();
    while let Some(end) = sse_event_end(bytes) {
        let event = parse_sse_event(&bytes[..end]);
        bytes = &bytes[end..];
        if event.has_data && !event.valid {
            return Err(invalid_stream());
        }
        if let Some(payload) = &event.payload {
            if completed.is_some() {
                return Err(invalid_stream());
            }
            if event.outcome.is_none() {
                capture.observe(payload);
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
        if let Some(response) = event
            .response
            .filter(|_| event.outcome == Some(TerminalOutcome::Success))
        {
            if completed.is_some() {
                return Err(invalid_stream());
            }
            completed = Some(response);
        }
    }
    if !bytes.trim_ascii().is_empty() {
        return Err(invalid_stream());
    }
    let mut response = completed.ok_or_else(invalid_stream)?;
    if response
        .get("status")
        .is_some_and(|value| value != "completed")
    {
        return Err(invalid_stream());
    }
    if response
        .get("output")
        .is_none_or(|output| output.as_array().is_some_and(Vec::is_empty))
    {
        response = capture
            .finish(Some(response), None)
            .ok_or_else(invalid_stream)?;
    }
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(invalid_stream)?;
    let compactions = output
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("compaction"))
        .collect::<Vec<_>>();
    if compactions.len() != 1
        || compactions[0]
            .get("encrypted_content")
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    {
        return Err(invalid_stream());
    }
    let mut result = json!({"object": "response.compaction", "output": output});
    for key in ["id", "created_at", "usage"] {
        if let Some(value) = response.get(key) {
            result[key] = value.clone();
        }
    }
    serde_json::to_vec(&result).map_err(|_| invalid_stream())
}

// Only an explicit missing legacy endpoint permits this single compatibility
// attempt. Its generation or malformed response must never be replayed.
pub(super) async fn execute(
    runtime: &GatewayRuntime,
    route: &mut ExecutorRoute,
    request: &Value,
    headers: &HeaderMap,
    scope: Option<&CodexTurnStateScope<'_>>,
) -> Result<(HeaderMap, Vec<u8>), Box<(AttemptFailure, HeaderMap)>> {
    let body = request_body(request).map_err(|failure| Box::new((failure, HeaderMap::new())))?;
    let upstream = runtime
        .send_authorized_request(
            &route.candidate_id,
            runtime
                .request_client(&route.candidate_id)
                .post(route.upstream_url.clone())
                .headers(headers.clone())
                .header(CONTENT_TYPE, "application/json")
                .header(ACCEPT, "text/event-stream")
                .body(body),
            codex_client_version(headers),
            scope,
        )
        .await
        .map_err(|error| Box::new((AttemptFailure::authorized_request(error), HeaderMap::new())))?;
    route.account_token_generation = upstream.account_token_generation;
    let response = upstream.response;
    let status = response.status();
    let mut headers = response.headers().clone();
    let bytes =
        crate::transport::collect_limited(response, crate::runtime::MAX_NON_STREAM_BODY_BYTES)
            .await
            .map_err(|_| Box::new((invalid_stream(), headers.clone())))?;
    if !status.is_success() {
        return Err(Box::new((
            AttemptFailure::status_with_body(status, Some(&bytes)),
            headers,
        )));
    }
    let body = compact_output(&bytes).map_err(|failure| Box::new((failure, headers.clone())))?;
    for name in [
        "content-length",
        "content-encoding",
        "transfer-encoding",
        "trailer",
    ] {
        headers.remove(name);
    }
    headers.insert(CONTENT_TYPE, "application/json".parse().unwrap());
    Ok((headers, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_preserves_history_and_request_fields() {
        let request = json!({"model":"future-model", "input":[{"type":"function_call_output","call_id":"call-1","output":"result"}], "tools":[{"type":"function","name":"test"}], "reasoning":{"effort":"high"}, "extension":true});
        let body: Value = serde_json::from_slice(&request_body(&request).ok().unwrap()).unwrap();
        assert_eq!(body["input"][0], request["input"][0]);
        assert_eq!(body["tools"], request["tools"]);
        assert_eq!(body["reasoning"], request["reasoning"]);
        assert_eq!(body["extension"], true);
        assert_eq!(body["input"][1]["type"], "compaction_trigger");
        let again: Value = serde_json::from_slice(&request_body(&body).ok().unwrap()).unwrap();
        assert_eq!(again, body);
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
        let body = format!(
            "data: {{\"type\":\"response.created\",\"response\":{{\"id\":\"synthetic-compaction\"}}}}\r\n\r\n: keep-alive\n\ndata: {terminal}\n\n"
        );
        let response: Value =
            serde_json::from_slice(&compact_output(body.as_bytes()).ok().unwrap()).unwrap();
        assert_eq!(response["output"], output);
        assert_eq!(response["usage"]["input_tokens"], 100);
        assert_eq!(response["usage"]["output_tokens"], 8);
    }

    #[test]
    fn compaction_requires_terminal_success_and_real_encrypted_output() {
        let complete = b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"compaction\",\"encrypted_content\":\"synthetic\"}],\"usage\":{\"input_tokens\":42}}}\n\n";
        let body: Value = serde_json::from_slice(&compact_output(complete).ok().unwrap()).unwrap();
        assert_eq!(body["usage"]["input_tokens"], 42);
        assert!(body["usage"].get("output_tokens").is_none());
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
        let result: Value =
            serde_json::from_slice(&compact_output(complete.as_bytes()).ok().unwrap()).unwrap();
        assert_eq!(result["output"], json!([message, compaction]));
        assert_eq!(result["usage"], json!({"input_tokens":12}));
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

use super::contracts::{AdapterError, AdapterResult};
use serde_json::{json, Map, Value};

/// A complete Responses-to-Gemini request. Gemini selects the model from the
/// URL, so the body deliberately has no `model` field.
#[derive(Clone, Debug)]
pub struct GeminiBridgeRequest {
    pub(super) upstream_body: Value,
    pub(super) model: String,
    pub(super) response_id: String,
}

impl GeminiBridgeRequest {
    pub fn upstream_body(&self) -> &Value {
        &self.upstream_body
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn response_id(&self) -> &str {
        &self.response_id
    }
}

#[derive(Clone, Debug)]
pub struct GeminiBridgeResponse {
    pub response_body: Value,
    pub response_id: String,
}

/// Converts the text-only subset of Responses into Gemini's native
/// `generateContent` contract. Tools, images, reasoning, and continuation
/// carry semantics that require their own capability probes, so they are
/// rejected before Relay sends an upstream request.
pub fn prepare_responses_to_gemini(
    request: &Value,
    model: &str,
    _stream: bool,
    _response_scope: &str,
    response_id_seed: &str,
) -> AdapterResult<GeminiBridgeRequest> {
    let object = request
        .as_object()
        .ok_or_else(AdapterError::invalid_request)?;
    reject_unsupported_fields(object)?;
    let mut system_parts = Vec::new();
    if let Some(instructions) = object.get("instructions") {
        system_parts.extend(content_parts(instructions)?);
    }
    let mut contents = Vec::new();
    append_input_contents(
        &mut contents,
        &mut system_parts,
        object
            .get("input")
            .ok_or_else(AdapterError::invalid_request)?,
    )?;
    if contents.is_empty() {
        return Err(AdapterError::invalid_request());
    }

    let mut body = Map::from_iter([("contents".to_string(), Value::Array(contents))]);
    if !system_parts.is_empty() {
        body.insert(
            "systemInstruction".to_string(),
            json!({"parts": system_parts}),
        );
    }
    let mut generation = Map::new();
    copy_number(object, "temperature", "temperature", &mut generation)?;
    copy_number(object, "top_p", "topP", &mut generation)?;
    copy_number(
        object,
        "max_output_tokens",
        "maxOutputTokens",
        &mut generation,
    )?;
    if let Some(stop) = object.get("stop") {
        generation.insert(
            "stopSequences".to_string(),
            Value::Array(stop_sequences(stop)?),
        );
    }
    if !generation.is_empty() {
        body.insert("generationConfig".to_string(), Value::Object(generation));
    }

    let seed = response_id_seed
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>();
    if seed.is_empty() {
        return Err(AdapterError::invalid_request());
    }
    Ok(GeminiBridgeRequest {
        upstream_body: Value::Object(body),
        model: model.to_string(),
        response_id: format!("gemini_bridge_{seed}"),
    })
}

pub fn translate_gemini_response(
    request: GeminiBridgeRequest,
    upstream: &Value,
) -> AdapterResult<GeminiBridgeResponse> {
    let text = response_text(upstream)?;
    let response_id = request.response_id;
    let response_body = responses_body(&response_id, &request.model, &text, upstream);
    Ok(GeminiBridgeResponse {
        response_body,
        response_id,
    })
}

pub(super) fn response_text(upstream: &Value) -> AdapterResult<String> {
    let candidate = upstream
        .get("candidates")
        .and_then(Value::as_array)
        .and_then(|candidates| candidates.first())
        .ok_or_else(AdapterError::upstream_response_invalid)?;
    let parts = candidate
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .ok_or_else(AdapterError::upstream_response_invalid)?;
    let mut text = String::new();
    for part in parts {
        let part = part
            .as_object()
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        if part.get("thought").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let value = part
            .get("text")
            .and_then(Value::as_str)
            .ok_or_else(AdapterError::upstream_response_invalid)?;
        text.push_str(value);
    }
    (!text.is_empty())
        .then_some(text)
        .ok_or_else(AdapterError::upstream_response_invalid)
}

pub(super) fn responses_body(
    response_id: &str,
    model: &str,
    text: &str,
    upstream: &Value,
) -> Value {
    json!({
        "id": response_id,
        "object": "response",
        "created_at": 0,
        "status": "completed",
        "model": model,
        "output": [{
            "id": "gemini_bridge_output",
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [{"type": "output_text", "text": text, "annotations": []}],
        }],
        "usage": responses_usage(upstream.get("usageMetadata")),
    })
}

pub(super) fn responses_usage(usage: Option<&Value>) -> Value {
    let input = usage
        .and_then(|usage| usage.get("promptTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let output = usage
        .and_then(|usage| usage.get("candidatesTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let total = usage
        .and_then(|usage| usage.get("totalTokenCount"))
        .and_then(Value::as_u64)
        .unwrap_or_else(|| input.saturating_add(output));
    let mut result = Map::from_iter([
        ("input_tokens".to_string(), Value::from(input)),
        ("output_tokens".to_string(), Value::from(output)),
        ("total_tokens".to_string(), Value::from(total)),
    ]);
    if let Some(cached) = usage
        .and_then(|usage| usage.get("cachedContentTokenCount"))
        .and_then(Value::as_u64)
    {
        result.insert(
            "input_tokens_details".to_string(),
            json!({"cached_tokens": cached}),
        );
    }
    if let Some(reasoning) = usage
        .and_then(|usage| usage.get("thoughtsTokenCount"))
        .and_then(Value::as_u64)
    {
        result.insert(
            "output_tokens_details".to_string(),
            json!({"reasoning_tokens": reasoning}),
        );
    }
    Value::Object(result)
}

fn reject_unsupported_fields(request: &Map<String, Value>) -> AdapterResult<()> {
    for key in [
        "previous_response_id",
        "tools",
        "tool_choice",
        "parallel_tool_calls",
        "include",
        "reasoning",
        "background",
    ] {
        if request.get(key).is_some_and(|value| !value.is_null()) {
            return Err(AdapterError::unsupported_binding());
        }
    }
    Ok(())
}

fn append_input_contents(
    contents: &mut Vec<Value>,
    system_parts: &mut Vec<Value>,
    input: &Value,
) -> AdapterResult<()> {
    match input {
        Value::String(text) if !text.is_empty() => {
            contents.push(json!({"role": "user", "parts": [{"text": text}]}));
            Ok(())
        }
        Value::Array(items) => {
            for item in items {
                let item = item.as_object().ok_or_else(AdapterError::invalid_request)?;
                let role = item
                    .get("role")
                    .and_then(Value::as_str)
                    .ok_or_else(AdapterError::invalid_request)?;
                let content = item
                    .get("content")
                    .ok_or_else(AdapterError::invalid_request)?;
                let parts = content_parts(content)?;
                if parts.is_empty() {
                    continue;
                }
                match role {
                    "user" => contents.push(json!({"role": "user", "parts": parts})),
                    "assistant" => contents.push(json!({"role": "model", "parts": parts})),
                    "developer" | "system" => system_parts.extend(parts),
                    _ => return Err(AdapterError::invalid_request()),
                }
            }
            Ok(())
        }
        _ => Err(AdapterError::invalid_request()),
    }
}

fn content_parts(content: &Value) -> AdapterResult<Vec<Value>> {
    match content {
        Value::String(text) if !text.is_empty() => Ok(vec![json!({"text": text})]),
        Value::String(_) => Ok(Vec::new()),
        Value::Array(parts) => parts
            .iter()
            .map(|part| {
                let part = part.as_object().ok_or_else(AdapterError::invalid_request)?;
                match part.get("type").and_then(Value::as_str) {
                    Some("input_text" | "output_text" | "text") => part
                        .get("text")
                        .and_then(Value::as_str)
                        .map(|text| json!({"text": text}))
                        .ok_or_else(AdapterError::invalid_request),
                    _ => Err(AdapterError::unsupported_binding()),
                }
            })
            .collect(),
        _ => Err(AdapterError::invalid_request()),
    }
}

fn copy_number(
    input: &Map<String, Value>,
    source: &str,
    target: &str,
    output: &mut Map<String, Value>,
) -> AdapterResult<()> {
    let Some(value) = input.get(source) else {
        return Ok(());
    };
    if !value.is_number() {
        return Err(AdapterError::invalid_request());
    }
    output.insert(target.to_string(), value.clone());
    Ok(())
}

fn stop_sequences(value: &Value) -> AdapterResult<Vec<Value>> {
    match value {
        Value::String(value) if !value.is_empty() => Ok(vec![Value::String(value.clone())]),
        Value::Array(values) => {
            if values
                .iter()
                .any(|value| !value.as_str().is_some_and(|value| !value.is_empty()))
            {
                return Err(AdapterError::invalid_request());
            }
            Ok(values.clone())
        }
        _ => Err(AdapterError::invalid_request()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_text_becomes_native_gemini_request_and_usage() {
        let request = prepare_responses_to_gemini(
            &json!({
                "instructions": "Be concise.",
                "input": [
                    {"role": "user", "content": [{"type": "input_text", "text": "Hello"}]},
                    {"role": "assistant", "content": [{"type": "output_text", "text": "Hi"}]},
                    {"role": "user", "content": "Continue"}
                ],
                "temperature": 0.2,
                "max_output_tokens": 32,
            }),
            "gemini-test",
            false,
            "route",
            "request-42",
        )
        .unwrap();
        assert_eq!(
            request.upstream_body["systemInstruction"]["parts"][0]["text"],
            "Be concise."
        );
        assert_eq!(request.upstream_body["contents"][1]["role"], "model");
        assert_eq!(
            request.upstream_body["generationConfig"]["maxOutputTokens"],
            32
        );

        let response = translate_gemini_response(
            request,
            &json!({
                "candidates": [{"content": {"parts": [{"text": "Done."}]}}],
                "usageMetadata": {
                    "promptTokenCount": 4,
                    "candidatesTokenCount": 3,
                    "cachedContentTokenCount": 2,
                    "thoughtsTokenCount": 1,
                    "totalTokenCount": 7,
                }
            }),
        )
        .unwrap();
        assert_eq!(
            response.response_body["output"][0]["content"][0]["text"],
            "Done."
        );
        assert_eq!(
            response.response_body["usage"]["input_tokens_details"]["cached_tokens"],
            2
        );
        assert_eq!(
            response.response_body["usage"]["output_tokens_details"]["reasoning_tokens"],
            1
        );
    }

    #[test]
    fn unsupported_responses_features_are_rejected_before_network_io() {
        let error = prepare_responses_to_gemini(
            &json!({"input": "Hello", "tools": []}),
            "gemini-test",
            false,
            "route",
            "request-42",
        )
        .unwrap_err();
        assert_eq!(error.code(), "adapter_binding_unsupported");
    }
}

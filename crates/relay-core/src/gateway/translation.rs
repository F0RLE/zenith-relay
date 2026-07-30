use super::errors::AttemptFailure;
use axum::body::Bytes;
use serde_json::{json, Value};

pub(super) fn translate_responses_request(
    request: &Value,
    model: &str,
    stream: bool,
) -> Result<Vec<u8>, AttemptFailure> {
    let object = request
        .as_object()
        .ok_or_else(AttemptFailure::invalid_request)?;
    if object
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(AttemptFailure::invalid_request());
    }

    let mut messages = Vec::new();
    if let Some(instructions) = object.get("instructions") {
        let Some(instructions) = instructions.as_str() else {
            return Err(AttemptFailure::invalid_request());
        };
        if !instructions.trim().is_empty() {
            messages.push(json!({"role": "system", "content": instructions}));
        }
    }
    let input = object
        .get("input")
        .ok_or_else(AttemptFailure::invalid_request)?;
    match input {
        Value::String(text) => messages.push(json!({"role": "user", "content": text})),
        Value::Array(items)
            if !items.is_empty()
                && items
                    .iter()
                    .all(|item| item.get("role").and_then(Value::as_str).is_some()) =>
        {
            for item in items {
                let role = item
                    .get("role")
                    .and_then(Value::as_str)
                    .ok_or_else(AttemptFailure::invalid_request)?;
                if !matches!(role, "developer" | "system" | "user" | "assistant" | "tool") {
                    return Err(AttemptFailure::invalid_request());
                }
                let content = translate_message_content(
                    item.get("content")
                        .ok_or_else(AttemptFailure::invalid_request)?,
                )?;
                messages.push(json!({"role": role, "content": content}));
            }
        }
        Value::Array(items) if !items.is_empty() => messages.push(json!({
            "role": "user",
            "content": translate_message_content(input)?,
        })),
        _ => return Err(AttemptFailure::invalid_request()),
    }

    let mut translated = serde_json::Map::from_iter([
        ("model".to_string(), Value::String(model.to_string())),
        ("messages".to_string(), Value::Array(messages)),
        ("stream".to_string(), Value::Bool(stream)),
    ]);
    for field in [
        "temperature",
        "top_p",
        "stop",
        "parallel_tool_calls",
        "service_tier",
    ] {
        if let Some(value) = object.get(field) {
            translated.insert(field.to_string(), value.clone());
        }
    }
    if let Some(value) = object.get("max_output_tokens") {
        translated.insert("max_completion_tokens".to_string(), value.clone());
    }
    if let Some(tools) = object.get("tools") {
        translated.insert("tools".to_string(), translate_tools(tools)?);
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        translated.insert(
            "tool_choice".to_string(),
            translate_tool_choice(tool_choice)?,
        );
    }
    serde_json::to_vec(&Value::Object(translated)).map_err(|_| AttemptFailure::invalid_request())
}

pub(super) fn translate_chat_request(
    request: &Value,
    model: &str,
    stream: bool,
) -> Result<Vec<u8>, AttemptFailure> {
    let object = request
        .as_object()
        .ok_or_else(AttemptFailure::invalid_request)?;
    let messages = object
        .get("messages")
        .and_then(Value::as_array)
        .filter(|messages| !messages.is_empty())
        .ok_or_else(AttemptFailure::invalid_request)?;
    let mut input = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(AttemptFailure::invalid_request)?;
        if role == "tool" {
            let call_id = message
                .get("tool_call_id")
                .and_then(Value::as_str)
                .ok_or_else(AttemptFailure::invalid_request)?;
            let output = message
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(AttemptFailure::invalid_request)?;
            input.push(json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output,
            }));
            continue;
        }
        if !matches!(role, "developer" | "system" | "user" | "assistant") {
            return Err(AttemptFailure::invalid_request());
        }
        if let Some(content) = message.get("content").filter(|content| !content.is_null()) {
            input.push(json!({
                "role": role,
                "content": translate_chat_message_content(content)?,
            }));
        }
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in tool_calls {
                let function = call
                    .get("function")
                    .and_then(Value::as_object)
                    .ok_or_else(AttemptFailure::invalid_request)?;
                input.push(json!({
                    "type": "function_call",
                    "call_id": call.get("id").and_then(Value::as_str).ok_or_else(AttemptFailure::invalid_request)?,
                    "name": function.get("name").and_then(Value::as_str).ok_or_else(AttemptFailure::invalid_request)?,
                    "arguments": function.get("arguments").and_then(Value::as_str).unwrap_or("{}"),
                }));
            }
        }
    }
    if input.is_empty() {
        return Err(AttemptFailure::invalid_request());
    }

    let mut translated = serde_json::Map::from_iter([
        ("model".to_string(), Value::String(model.to_string())),
        ("input".to_string(), Value::Array(input)),
        ("stream".to_string(), Value::Bool(stream)),
    ]);
    for field in [
        "temperature",
        "top_p",
        "parallel_tool_calls",
        "service_tier",
    ] {
        if let Some(value) = object.get(field) {
            translated.insert(field.to_string(), value.clone());
        }
    }
    if let Some(value) = object
        .get("max_completion_tokens")
        .or_else(|| object.get("max_tokens"))
    {
        translated.insert("max_output_tokens".to_string(), value.clone());
    }
    if let Some(tools) = object.get("tools") {
        translated.insert("tools".to_string(), translate_chat_tools(tools)?);
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        translated.insert(
            "tool_choice".to_string(),
            translate_chat_tool_choice(tool_choice)?,
        );
    }
    serde_json::to_vec(&Value::Object(translated)).map_err(|_| AttemptFailure::invalid_request())
}

fn translate_chat_message_content(content: &Value) -> Result<Value, AttemptFailure> {
    match content {
        Value::String(_) => Ok(content.clone()),
        Value::Array(items) => items
            .iter()
            .map(|item| match item.get("type").and_then(Value::as_str) {
                Some("text") => item
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|text| json!({"type": "input_text", "text": text}))
                    .ok_or_else(AttemptFailure::invalid_request),
                Some("image_url") => item
                    .get("image_url")
                    .and_then(|image| image.get("url"))
                    .and_then(Value::as_str)
                    .map(|url| json!({"type": "input_image", "image_url": url}))
                    .ok_or_else(AttemptFailure::invalid_request),
                _ => Err(AttemptFailure::invalid_request()),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        _ => Err(AttemptFailure::invalid_request()),
    }
}

fn translate_chat_tools(tools: &Value) -> Result<Value, AttemptFailure> {
    let tools = tools
        .as_array()
        .ok_or_else(AttemptFailure::invalid_request)?;
    tools
        .iter()
        .map(|tool| {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return Err(AttemptFailure::invalid_request());
            }
            let function = tool
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(AttemptFailure::invalid_request)?;
            let mut translated = serde_json::Map::from_iter([
                ("type".to_string(), Value::String("function".to_string())),
                (
                    "name".to_string(),
                    function
                        .get("name")
                        .cloned()
                        .ok_or_else(AttemptFailure::invalid_request)?,
                ),
            ]);
            for field in ["description", "parameters", "strict"] {
                if let Some(value) = function.get(field) {
                    translated.insert(field.to_string(), value.clone());
                }
            }
            Ok(Value::Object(translated))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn translate_chat_tool_choice(tool_choice: &Value) -> Result<Value, AttemptFailure> {
    if tool_choice.is_string() {
        return Ok(tool_choice.clone());
    }
    let name = tool_choice
        .get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(AttemptFailure::invalid_request)?;
    Ok(json!({"type": "function", "name": name}))
}

fn translate_message_content(content: &Value) -> Result<Value, AttemptFailure> {
    match content {
        Value::String(_) => Ok(content.clone()),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if let Some(text) = item.as_str() {
                    return Ok(json!({"type": "text", "text": text}));
                }
                let kind = item
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(AttemptFailure::invalid_request)?;
                match kind {
                    "input_text" | "output_text" | "text" => item
                        .get("text")
                        .and_then(Value::as_str)
                        .map(|text| json!({"type": "text", "text": text}))
                        .ok_or_else(AttemptFailure::invalid_request),
                    "input_image" => item
                        .get("image_url")
                        .or_else(|| item.get("url"))
                        .and_then(Value::as_str)
                        .map(|url| json!({"type": "image_url", "image_url": {"url": url}}))
                        .ok_or_else(AttemptFailure::invalid_request),
                    _ => Err(AttemptFailure::invalid_request()),
                }
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        _ => Err(AttemptFailure::invalid_request()),
    }
}

fn translate_tools(tools: &Value) -> Result<Value, AttemptFailure> {
    let tools = tools
        .as_array()
        .ok_or_else(AttemptFailure::invalid_request)?;
    tools
        .iter()
        .map(|tool| {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return Err(AttemptFailure::invalid_request());
            }
            if let Some(function) = tool.get("function") {
                return Ok(json!({"type": "function", "function": function}));
            }
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(AttemptFailure::invalid_request)?;
            let mut function =
                serde_json::Map::from_iter([("name".to_string(), Value::String(name.to_string()))]);
            for field in ["description", "parameters", "strict"] {
                if let Some(value) = tool.get(field) {
                    function.insert(field.to_string(), value.clone());
                }
            }
            Ok(json!({"type": "function", "function": function}))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn translate_tool_choice(tool_choice: &Value) -> Result<Value, AttemptFailure> {
    if tool_choice.is_string() {
        return Ok(tool_choice.clone());
    }
    let name = tool_choice
        .get("name")
        .or_else(|| {
            tool_choice
                .get("function")
                .and_then(|value| value.get("name"))
        })
        .and_then(Value::as_str)
        .ok_or_else(AttemptFailure::invalid_request)?;
    Ok(json!({"type": "function", "function": {"name": name}}))
}

pub(super) fn translate_chat_response(body: &[u8]) -> Result<Vec<u8>, AttemptFailure> {
    let response: Value =
        serde_json::from_slice(body).map_err(|_| AttemptFailure::translation())?;
    let choice = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(AttemptFailure::translation)?;
    let message = choice
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(AttemptFailure::translation)?;
    let id = response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chat-response");
    let model = response
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut output = Vec::new();
    if let Some(content) = message.get("content").and_then(Value::as_str) {
        output.push(json!({
            "id": format!("{id}-message"),
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [{"type": "output_text", "text": content, "annotations": []}],
        }));
    }
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        let choice_index = choice.get("index").and_then(Value::as_u64).unwrap_or(0);
        for (tool_index, call) in tool_calls.iter().enumerate() {
            let function = call
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(AttemptFailure::translation)?;
            let call_id = call
                .get("id")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("call_{id}_{choice_index}_{tool_index}"));
            output.push(json!({
                "id": call_id,
                "type": "function_call",
                "status": "completed",
                "call_id": call_id,
                "name": function.get("name").and_then(Value::as_str).ok_or_else(AttemptFailure::translation)?,
                "arguments": function.get("arguments").and_then(Value::as_str).unwrap_or("{}"),
            }));
        }
    }
    if output.is_empty() {
        return Err(AttemptFailure::translation());
    }
    let usage = response.get("usage").map(|usage| {
        json!({
            "input_tokens": usage.get("prompt_tokens").or_else(|| usage.get("input_tokens")).and_then(Value::as_u64).unwrap_or(0),
            "output_tokens": usage.get("completion_tokens").or_else(|| usage.get("output_tokens")).and_then(Value::as_u64).unwrap_or(0),
            "total_tokens": usage.get("total_tokens").and_then(Value::as_u64).unwrap_or(0),
        })
    });
    serde_json::to_vec(&json!({
        "id": id,
        "object": "response",
        "created_at": response.get("created").and_then(Value::as_u64).unwrap_or(0),
        "status": "completed",
        "model": model,
        "output": output,
        "usage": usage,
    }))
    .map_err(|_| AttemptFailure::translation())
}

pub(super) fn translate_responses_response(body: &[u8]) -> Result<Vec<u8>, AttemptFailure> {
    let response: Value =
        serde_json::from_slice(body).map_err(|_| AttemptFailure::translation())?;
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(AttemptFailure::translation)?;
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        if matches!(
                            part.get("type").and_then(Value::as_str),
                            Some("output_text" | "text")
                        ) {
                            if let Some(value) = part.get("text").and_then(Value::as_str) {
                                text.push_str(value);
                            }
                        }
                    }
                }
            }
            Some("function_call") => {
                let call_id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .ok_or_else(AttemptFailure::translation)?;
                tool_calls.push(json!({
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": item.get("name").and_then(Value::as_str).ok_or_else(AttemptFailure::translation)?,
                        "arguments": item.get("arguments").and_then(Value::as_str).unwrap_or("{}"),
                    }
                }));
            }
            _ => {}
        }
    }
    if text.is_empty() && tool_calls.is_empty() {
        return Err(AttemptFailure::translation());
    }
    let mut message = serde_json::Map::from_iter([
        ("role".to_string(), Value::String("assistant".to_string())),
        (
            "content".to_string(),
            if text.is_empty() {
                Value::Null
            } else {
                Value::String(text)
            },
        ),
    ]);
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    let usage = response.get("usage").map(|usage| {
        json!({
            "prompt_tokens": usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            "completion_tokens": usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
            "total_tokens": usage.get("total_tokens").and_then(Value::as_u64).unwrap_or(0),
        })
    });
    serde_json::to_vec(&json!({
        "id": response.get("id").and_then(Value::as_str).unwrap_or("response"),
        "object": "chat.completion",
        "created": response.get("created_at").and_then(Value::as_u64).unwrap_or(0),
        "model": response.get("model").and_then(Value::as_str).unwrap_or("unknown"),
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": if response.get("status").and_then(Value::as_str) == Some("incomplete") { "length" } else if response.get("output").and_then(Value::as_array).is_some_and(|output| output.iter().any(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))) { "tool_calls" } else { "stop" },
        }],
        "usage": usage,
    }))
    .map_err(|_| AttemptFailure::translation())
}

pub(super) fn completed_sse(response: &[u8]) -> Bytes {
    let response = serde_json::from_slice::<Value>(response).unwrap_or(Value::Null);
    Bytes::from(format!(
        "data: {}\n\n",
        json!({"type": "response.completed", "response": response})
    ))
}

pub(super) fn completed_chat_sse(response: &[u8]) -> Bytes {
    let Ok(response) = serde_json::from_slice::<Value>(response) else {
        return Bytes::new();
    };
    let choice = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .cloned()
        .unwrap_or(Value::Null);
    let common = json!({
        "id": response.get("id").cloned().unwrap_or(Value::Null),
        "object": "chat.completion.chunk",
        "created": response.get("created").cloned().unwrap_or(Value::from(0)),
        "model": response.get("model").cloned().unwrap_or(Value::Null),
    });
    let mut first = common.clone();
    first["choices"] = json!([{
        "index": 0,
        "delta": choice.get("message").cloned().unwrap_or(Value::Null),
        "finish_reason": Value::Null,
    }]);
    let mut terminal = common;
    terminal["choices"] = json!([{
        "index": 0,
        "delta": {},
        "finish_reason": choice.get("finish_reason").cloned().unwrap_or(Value::String("stop".to_string())),
    }]);
    terminal["usage"] = response.get("usage").cloned().unwrap_or(Value::Null);
    Bytes::from(format!(
        "data: {first}\n\ndata: {terminal}\n\ndata: [DONE]\n\n"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_translation_synthesizes_missing_tool_call_ids() {
        let translated = translate_chat_response(
            br#"{"id":"chatcmpl_1","model":"model","choices":[{"index":2,"message":{"tool_calls":[{"type":"function","function":{"name":"lookup","arguments":"{}"}}]}}]}"#,
        )
        .unwrap_or_else(|_| panic!("chat response should translate"));
        let response: Value = serde_json::from_slice(&translated).unwrap();

        assert_eq!(
            response.pointer("/output/0/id").unwrap(),
            "call_chatcmpl_1_2_0"
        );
        assert_eq!(
            response.pointer("/output/0/call_id").unwrap(),
            "call_chatcmpl_1_2_0"
        );
    }
}

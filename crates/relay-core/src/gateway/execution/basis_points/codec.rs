use crate::protocol::AdapterError;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

pub(super) fn as_input_items(request_input: Option<&Value>) -> Vec<Value> {
    match request_input {
        Some(Value::String(text)) => vec![json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": text}],
        })],
        Some(Value::Object(input_fields)) => vec![Value::Object(input_fields.clone())],
        Some(Value::Array(items)) => items.clone(),
        _ => Vec::new(),
    }
}

pub(super) fn text_message(role: &str, content_type: &str, text: String) -> Value {
    json!({
        "type": "message",
        "role": role,
        "content": [{"type": content_type, "text": text}],
    })
}

pub(super) fn explicit_input_message(mut item: Value) -> Value {
    let Some(fields) = item.as_object_mut() else {
        return item;
    };
    let Some(role @ ("user" | "assistant" | "system" | "developer")) =
        fields.get("role").and_then(Value::as_str)
    else {
        return item;
    };
    let content_type = if role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    match fields.get("type") {
        None => {
            fields.insert("type".into(), Value::String("message".into()));
        }
        Some(Value::String(kind)) if kind == "message" => {}
        _ => return item,
    }
    // Responses accepts compact messages without a discriminator and with
    // string content. BPS needs explicit message/content variants, including
    // for histories emitted by the shared cross-protocol codec.
    if let Some(Value::String(text)) = fields.get("content") {
        fields.insert("content".into(), json!([{"type":content_type,"text":text}]));
    }
    item
}

pub(super) fn json_text(json_value: &Value) -> Result<String, AdapterError> {
    serde_json::to_string(json_value).map_err(|_| AdapterError::invalid_request())
}

pub(super) fn encode_function_arguments(arguments: &Value) -> Result<String, AdapterError> {
    let fields = arguments
        .as_object()
        .ok_or_else(|| AdapterError::invalid_request().with_parameter("input.arguments"))?;
    let framed_field = fields
        .iter()
        .filter_map(|(field, value)| {
            let raw = value.as_str()?;
            let escapes = raw
                .bytes()
                .filter(|byte| matches!(*byte, b'"' | b'\\' | 0..=0x1f))
                .count();
            (escapes > 0).then_some((field, raw, escapes))
        })
        .max_by_key(|(_, _, escapes)| *escapes);
    let Some((field, raw, _)) = framed_field else {
        return json_text(arguments);
    };
    let mut other_arguments = fields.clone();
    other_arguments.remove(field);
    let header = json!({"field":field,"args":other_arguments});
    Ok(format!(
        "{}{}\n{raw}",
        super::FUNCTION_STRING_PREFIX,
        json_text(&header)?
    ))
}

pub(super) fn parse_payload_json(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(text).or_else(|error| {
        if !text.contains(['\n', '\r', '\t', '\\']) {
            return Err(error);
        }
        // Preserve non-JSON escapes literally; valid escapes retain JSON semantics.
        let mut normalized = String::with_capacity(text.len());
        let mut quoted = false;
        let mut escaped = false;
        let mut characters = text.chars().peekable();
        while let Some(character) = characters.next() {
            if quoted && !escaped {
                if character == '\\'
                    && characters.peek().is_some_and(|next| {
                        !next.is_control()
                            && !matches!(
                                *next,
                                '"' | '\\' | '/' | 'b' | 'f' | 'n' | 'r' | 't' | 'u'
                            )
                    })
                {
                    normalized.push_str("\\\\");
                    continue;
                }
                let escape = match character {
                    '\n' => Some("\\n"),
                    '\r' => Some("\\r"),
                    '\t' => Some("\\t"),
                    _ => None,
                };
                if let Some(escape) = escape {
                    normalized.push_str(escape);
                    continue;
                }
            }
            normalized.push(character);
            if character == '"' && !escaped {
                quoted = !quoted;
            }
            escaped = quoted && character == '\\' && !escaped;
        }
        serde_json::from_str(&normalized)
    })
}

pub(super) fn json_argument_error(text: &str, error: &serde_json::Error) -> AdapterError {
    let (failure, parameter) = if error.is_eof() {
        (
            AdapterError::upstream_tool_arguments_json_incomplete(),
            "output.run_officejs.args.json.eof",
        )
    } else {
        (
            AdapterError::upstream_tool_arguments_json_syntax_invalid(),
            "output.run_officejs.args.json.syntax",
        )
    };
    let reason = if error.is_eof() {
        "unexpected_eof"
    } else {
        let message = error.to_string();
        [
            ("invalid escape", "invalid_escape"),
            ("invalid unicode", "invalid_unicode"),
            ("control character", "unescaped_control"),
            ("expected `,`", "expected_separator"),
            ("expected `:`", "expected_colon"),
            ("key must be a string", "key_not_string"),
            ("trailing characters", "trailing_data"),
            ("trailing comma", "trailing_comma"),
            ("expected value", "expected_value"),
            ("expected ident", "invalid_literal"),
            ("recursion limit exceeded", "nesting_limit"),
        ]
        .into_iter()
        .find_map(|(prefix, reason)| message.starts_with(prefix).then_some(reason))
        .unwrap_or("syntax")
    };
    let trimmed = text.trim_start();
    let shape = if trimmed.starts_with(super::FUNCTION_STRING_PREFIX) {
        "string_frame"
    } else if trimmed.starts_with("```") {
        "markdown_fence"
    } else {
        match trimmed.chars().next() {
            Some('{') => "json_object",
            Some('[') => "json_array",
            Some('"') => "json_string",
            None => "empty",
            _ => "text",
        }
    };
    failure.with_parameter(parameter).with_json_diagnostic(
        reason,
        shape,
        error.line(),
        error.column(),
        text.len(),
    )
}

fn split_function_string_frame(frame: &str) -> Option<(&str, &str)> {
    let mut depth = 0_usize;
    let mut quoted = false;
    let mut escaped = false;
    for (offset, byte) in frame.bytes().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
            continue;
        }
        match byte {
            b'"' => quoted = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth = depth.saturating_sub(1),
            b'\n' if depth == 0 => return Some((&frame[..offset], &frame[offset + 1..])),
            _ => {}
        }
    }
    // Keep the original failure path for malformed or incomplete headers. This
    // scan only finds a delimiter outside JSON containers/strings; the existing
    // decoder and original tool schema still validate the whole header. Never
    // normalize, trim, or parse the executable body after that delimiter.
    frame.split_once('\n')
}

pub(super) fn function_arguments(payload: &Value) -> Result<Value, AdapterError> {
    let arguments = match payload {
        Value::Object(_) => payload.clone(),
        Value::String(text) => {
            if let Some(frame) = text.strip_prefix(super::FUNCTION_STRING_PREFIX) {
                let invalid = AdapterError::upstream_tool_arguments_frame_invalid();
                let (header, raw) = split_function_string_frame(frame).ok_or_else(|| {
                    invalid.with_parameter("output.run_officejs.args.frame.separator")
                })?;
                let header = parse_payload_json(header).map_err(|error| {
                    invalid
                        .with_parameter("output.run_officejs.args.frame.header")
                        .with_diagnostics_from(json_argument_error(header, &error))
                })?;
                let fields = header
                    .as_object()
                    .filter(|fields| fields.len() == 2)
                    .ok_or_else(|| {
                        invalid
                            .with_parameter("output.run_officejs.args.frame.header")
                            .with_shape_diagnostic(&header, None)
                    })?;
                let field = fields.get("field").and_then(Value::as_str).ok_or_else(|| {
                    invalid
                        .with_parameter("output.run_officejs.args.frame.field")
                        .with_shape_diagnostic(&header, Some(("field", &header["field"])))
                })?;
                let mut arguments = fields
                    .get("args")
                    .and_then(Value::as_object)
                    .cloned()
                    .ok_or_else(|| {
                        invalid
                            .with_parameter("output.run_officejs.args.frame.args")
                            .with_shape_diagnostic(&header, Some(("args", &header["args"])))
                    })?;
                if arguments.contains_key(field) {
                    return Err(
                        invalid.with_parameter("output.run_officejs.args.frame.duplicate_field")
                    );
                }
                arguments.insert(field.to_owned(), Value::String(raw.to_owned()));
                Value::Object(arguments)
            } else {
                parse_payload_json(text).map_err(|error| json_argument_error(text, &error))?
            }
        }
        _ => {
            return Err(AdapterError::upstream_tool_arguments_json_invalid()
                .with_parameter("output.run_officejs.args.object"))
        }
    };
    if !arguments.is_object() {
        return Err(AdapterError::upstream_tool_arguments_json_invalid()
            .with_parameter("output.run_officejs.args.object"));
    }
    Ok(arguments)
}

pub(super) fn parse_json_object(json_value: Option<&Value>) -> Option<Value> {
    match json_value {
        Some(Value::Object(object_fields)) => Some(Value::Object(object_fields.clone())),
        Some(Value::String(text)) => serde_json::from_str(text).ok(),
        _ => None,
    }
}

pub(super) fn parse_function_arguments(
    tool_call_object: &Map<String, Value>,
) -> Result<Value, AdapterError> {
    let raw_arguments = tool_call_object.get("arguments");
    let parsed = parse_json_object(raw_arguments).ok_or_else(AdapterError::invalid_request)?;
    if !parsed.is_object() {
        return Err(AdapterError::invalid_request().with_parameter("input.arguments"));
    }
    Ok(parsed)
}

/// Normalize client aliases without replacing an explicit level. The fresh
/// access catalog decides which levels this account accepts. An omitted level
/// leaves the provider's default intact.
pub(super) fn basis_points_reasoning_effort(request_body: &Map<String, Value>) -> Option<String> {
    request_body
        .get("reasoning")
        .and_then(Value::as_object)
        .and_then(|reasoning| reasoning.get("effort"))
        .or_else(|| request_body.get("reasoning_effort"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|effort_text| !effort_text.is_empty())
        .map(crate::providers::chatgpt::normalize_basis_points_reasoning_effort)
}

pub(super) fn explicit_conversation_key(request_fields: &Map<String, Value>) -> Option<String> {
    for key in [
        "prompt_cache_key",
        "promptCacheKey",
        "session_id",
        "sessionId",
    ] {
        if let Some(conversation_value) = request_fields
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|conversation_value| !conversation_value.is_empty())
        {
            return Some(conversation_value.to_string());
        }
    }
    request_fields
        .get("client_metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| {
            ["session_id", "sessionId"].iter().find_map(|key| {
                metadata
                    .get(*key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|conversation_value| !conversation_value.is_empty())
                    .map(str::to_string)
            })
        })
}

pub(super) fn short_hash(json_value: &impl serde::Serialize) -> String {
    let digest =
        crate::json_fingerprint::sha256(json_value).unwrap_or_else(|_| Sha256::digest([]).into());
    hex::encode(digest)
}

pub(super) fn stable_uuid(namespace: &str, identifier: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(namespace.as_bytes());
    digest.update([0]);
    digest.update(identifier.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.finalize()[..16]);
    // UUID version 5/variant bits keep the identifier accepted by the
    // Codex backend while the SHA-256 seed keeps it deterministic.
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

pub(super) fn request_iteration(input_items: &[Value]) -> String {
    let last_user = input_items
        .iter()
        .enumerate()
        .filter_map(|(index, input_item)| {
            (input_item.get("role").and_then(Value::as_str) == Some("user")).then_some(index)
        })
        .next_back()
        .unwrap_or(0);
    let iteration = input_items
        .iter()
        .skip(last_user.saturating_add(1))
        .filter(|input_item| {
            matches!(
                input_item.get("type").and_then(Value::as_str),
                Some("function_call_output" | "custom_tool_call_output")
            ) && !super::history::is_thread_handoff(input_item)
        })
        .count()
        .saturating_add(1);
    iteration.to_string()
}

pub(super) fn basis_points_metadata(
    request_fields: &Map<String, Value>,
    translated_input: &[Value],
) -> Value {
    let conversation = explicit_conversation_key(request_fields).unwrap_or_else(|| {
        translated_input
            .first()
            .map(short_hash)
            .unwrap_or_else(|| "anonymous".to_string())
    });
    let last_user = translated_input
        .iter()
        .enumerate()
        .filter_map(|(index, input_item)| {
            (input_item.get("role").and_then(Value::as_str) == Some("user")).then_some(index)
        })
        .next_back();
    let prefix_end = last_user.map_or(translated_input.len(), |index| index + 1);
    let turn_prefix = &translated_input[..prefix_end];
    let turn_fingerprint = short_hash(&turn_prefix);
    json!({
        "task_id": stable_uuid("cpa-oai-basispoints", &conversation),
        "turn_id": stable_uuid(
            "cpa-oai-basispoints/turn",
            &format!("{conversation}/{turn_fingerprint}")
        ),
        "agent_iteration": request_iteration(translated_input),
    })
}

use super::*;

pub(in crate::gateway) fn preserved_stream_error(value: &Value) -> Option<PreservedUpstreamError> {
    let event_type = value.get("type").and_then(Value::as_str);
    let category = upstream_event_failure_category(event_type, value)?;
    let status = upstream_status_from_value(value)
        .filter(|status| !status.is_success())
        .unwrap_or_else(|| upstream_failure_status(category));
    let failure = AttemptFailure::classified_with_hint(
        canonical_upstream_status(status, category),
        category,
        rate_limit_body_hint_value(value, SystemTime::now()),
    );
    preserved_upstream_error_value(&failure, value)
}

pub(in crate::gateway) fn rewrite_bridge_failure(
    bytes: Vec<u8>,
    preserved: Option<&PreservedUpstreamError>,
) -> Vec<u8> {
    let Some(preserved) = preserved else {
        return bytes;
    };
    let mut terminal = parse_sse_event(&bytes);
    if terminal.outcome != Some(TerminalOutcome::Failure) {
        return bytes;
    }
    let Some(error) = terminal
        .payload
        .as_mut()
        .and_then(|payload| payload.pointer_mut("/response/error"))
        .and_then(Value::as_object_mut)
    else {
        return bytes;
    };
    error.insert("code".to_string(), Value::String(preserved.code.clone()));
    error.insert(
        "message".to_string(),
        Value::String(preserved.message.clone()),
    );
    error.insert(
        "type".to_string(),
        Value::String(api_error_type(preserved.status, &preserved.code).to_string()),
    );
    let Some(payload) = terminal.payload else {
        return bytes;
    };
    let event_name = payload
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("response.failed");
    let Ok(payload) = serde_json::to_vec(&payload) else {
        return bytes;
    };
    let mut frame = Vec::with_capacity(payload.len() + event_name.len() + 16);
    frame.extend_from_slice(b"event: ");
    frame.extend_from_slice(event_name.as_bytes());
    frame.extend_from_slice(b"\ndata: ");
    frame.extend_from_slice(&payload);
    frame.extend_from_slice(b"\n\n");
    frame
}

#[derive(Default)]
pub(in crate::gateway) struct TerminalEvent {
    pub(in crate::gateway) has_data: bool,
    pub(in crate::gateway) valid: bool,
    pub(in crate::gateway) has_output_delta: bool,
    pub(in crate::gateway) outcome: Option<TerminalOutcome>,
    pub(in crate::gateway) error_status: Option<StatusCode>,
    pub(in crate::gateway) error_category: Option<&'static str>,
    pub(in crate::gateway) preserved_error: Option<PreservedUpstreamError>,
    pub(in crate::gateway) cooldown_hint: RateLimitBodyHint,
    pub(in crate::gateway) usage: Option<Value>,
    pub(in crate::gateway) applied_service_tier: Option<crate::ObservedServiceTier>,
    pub(in crate::gateway) response_id: Option<String>,
    pub(in crate::gateway) response: Option<Value>,
    pub(in crate::gateway) output_item: Option<Value>,
    pub(in crate::gateway) payload: Option<Value>,
}

#[derive(Debug, Eq, PartialEq)]
pub(in crate::gateway) enum TerminalOutcome {
    Success,
    Incomplete,
    Failure,
}

pub(in crate::gateway) fn parse_sse_event(event: &[u8]) -> TerminalEvent {
    let mut data = Vec::new();
    for line in event.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(value) = line.strip_prefix(b"data:") else {
            continue;
        };
        if !data.is_empty() {
            data.push(b'\n');
        }
        data.extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
    }
    if data.is_empty() {
        return TerminalEvent::default();
    }
    if data == b"[DONE]" {
        return TerminalEvent {
            has_data: true,
            valid: true,
            has_output_delta: false,
            outcome: Some(TerminalOutcome::Success),
            error_status: None,
            error_category: None,
            preserved_error: None,
            cooldown_hint: RateLimitBodyHint::default(),
            usage: None,
            applied_service_tier: None,
            response_id: None,
            response: None,
            output_item: None,
            payload: None,
        };
    }
    let Ok(value) = serde_json::from_slice::<Value>(&data) else {
        return TerminalEvent {
            has_data: true,
            ..TerminalEvent::default()
        };
    };
    let event_type = value.get("type").and_then(Value::as_str);
    let outcome = match event_type {
        Some("response.completed" | "response.done" | "message_stop") => {
            Some(TerminalOutcome::Success)
        }
        Some("response.failed" | "response.cancelled" | "response.canceled" | "error") => {
            Some(TerminalOutcome::Failure)
        }
        Some("response.incomplete") => Some(TerminalOutcome::Incomplete),
        _ => None,
    };
    let error_category = upstream_event_failure_category(event_type, &value);
    let error_status = error_category.map(|category| {
        let status = upstream_status_from_value(&value)
            .filter(|status| !status.is_success())
            .unwrap_or_else(|| upstream_failure_status(category));
        canonical_upstream_status(status, category)
    });
    let cooldown_hint = rate_limit_body_hint_value(&value, SystemTime::now());
    let preserved_error = preserved_stream_error(&value);
    let has_output_delta = has_output_delta(&value, event_type);
    let usage = find_usage(&value).cloned();
    let applied_service_tier = response_service_tier(&value);
    let response_id = response_id(&value).map(str::to_string);
    let response = value.get("response").cloned();
    let output_item = (value.get("type").and_then(Value::as_str)
        == Some("response.output_item.done"))
    .then(|| value.get("item").cloned())
    .flatten();
    TerminalEvent {
        has_data: true,
        valid: true,
        has_output_delta,
        outcome,
        error_status,
        error_category,
        preserved_error,
        cooldown_hint,
        usage,
        applied_service_tier,
        response_id,
        response,
        output_item,
        payload: Some(value),
    }
}

pub(in crate::gateway) fn has_output_delta(value: &Value, event_type: Option<&str>) -> bool {
    if matches!(
        event_type,
        Some(
            "response.output_text.delta"
                | "response.refusal.delta"
                | "response.function_call_arguments.delta"
                | "response.custom_tool_call_input.delta"
                | "response.mcp_call_arguments.delta"
                | "response.code_interpreter_call_code.delta"
        )
    ) && value
        .get("delta")
        .and_then(Value::as_str)
        .is_some_and(|delta| !delta.is_empty())
    {
        return true;
    }
    if event_type == Some("content_block_delta")
        && value.get("delta").is_some_and(|delta| {
            ["text", "partial_json"].into_iter().any(|key| {
                delta
                    .get(key)
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
            })
        })
    {
        return true;
    }
    if event_type == Some("response.output_item.added")
        && value
            .get("item")
            .is_some_and(output_item_has_meaningful_tool_call)
    {
        return true;
    }
    value
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| choices.iter().any(chat_choice_has_output_delta))
}

fn output_item_has_meaningful_tool_call(item: &Value) -> bool {
    matches!(
        item.get("type").and_then(Value::as_str),
        Some("function_call" | "custom_tool_call" | "mcp_call" | "computer_call")
    ) && ["call_id", "id", "name"].into_iter().any(|field| {
        item.get(field)
            .and_then(Value::as_str)
            .is_some_and(|value| !value.is_empty())
    })
}

fn chat_choice_has_output_delta(choice: &Value) -> bool {
    let Some(delta) = choice.get("delta") else {
        return false;
    };
    ["content", "refusal"].into_iter().any(|key| {
        delta
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
    }) || delta
        .get("function_call")
        .is_some_and(function_delta_has_output)
        || delta
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|calls| {
                calls.iter().any(|call| {
                    call.get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| !id.is_empty())
                        || call.get("function").is_some_and(function_delta_has_output)
                })
            })
}

fn function_delta_has_output(function: &Value) -> bool {
    ["name", "arguments"].into_iter().any(|key| {
        function
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
    })
}

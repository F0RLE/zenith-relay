use serde_json::Value;

/// Whether a decoded Responses event contains a compaction item. The item is
/// opaque state that must be forwarded unchanged and does not commit a route
/// before ordinary generation begins.
pub(in crate::gateway) fn is_compaction_payload(
    event_payload: &Value,
    event_type: Option<&str>,
) -> bool {
    event_type.is_some_and(crate::protocol::is_compaction_checkpoint_type)
        || event_type.is_some_and(is_opaque_compaction_event)
        || matches!(
            event_type,
            Some("response.output_item.added" | "response.output_item.done")
        ) && event_payload
            .get("item")
            .and_then(|output_item| output_item.get("type"))
            .and_then(Value::as_str)
            .is_some_and(crate::protocol::is_compaction_checkpoint_type)
}

/// Classifies protocol output without guessing about future event names.
/// Callers that have already forwarded a frame should treat an unknown event
/// conservatively; this function is deliberately limited to confirmed output.
pub(in crate::gateway) fn has_semantic_output(
    event_payload: &Value,
    event_type: Option<&str>,
) -> bool {
    !is_compaction_payload(event_payload, event_type)
        && (has_output_delta(event_payload, event_type)
            || event_type == Some("response.output_item.done")
            || [
                event_payload.get("output"),
                event_payload.pointer("/response/output"),
            ]
            .into_iter()
            .filter_map(|output_value| output_value.and_then(Value::as_array))
            .flatten()
            .any(|output_item| {
                !output_item
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(crate::protocol::is_compaction_checkpoint_type)
            }))
}

/// Known lifecycle and compaction frames are safe to buffer while selecting a
/// route. Anything else is left to the caller's conservative fallback.
pub(in crate::gateway) fn is_known_non_output_event(
    event_payload: &Value,
    event_type: Option<&str>,
) -> bool {
    is_compaction_payload(event_payload, event_type)
        || matches!(
            event_type,
            Some(
                "response.created"
                    | "response.in_progress"
                    | "response.queued"
                    | "codex.rate_limits"
                    | "codex.response.metadata"
            )
        )
}

pub(in crate::gateway) fn has_output_delta(
    event_payload: &Value,
    event_type: Option<&str>,
) -> bool {
    if event_type.is_some_and(is_responses_output_delta_type)
        && event_payload
            .get("delta")
            .and_then(Value::as_str)
            .is_some_and(|delta| !delta.is_empty())
    {
        return true;
    }
    if event_type == Some("content_block_delta")
        && event_payload.get("delta").is_some_and(|delta| {
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
        && event_payload
            .get("item")
            .is_some_and(output_item_has_meaningful_tool_call)
    {
        return true;
    }
    event_payload
        .get("choices")
        .and_then(Value::as_array)
        .is_some_and(|choices| choices.iter().any(chat_choice_has_output_delta))
        || event_payload
            .get("candidates")
            .and_then(Value::as_array)
            .is_some_and(|candidates| candidates.iter().any(gemini_candidate_has_output_delta))
}

/// Responses delta names that carry generated output. The full parser and the
/// fast SSE path share this list so a new event cannot be handled by only one.
pub(in crate::gateway) fn is_responses_output_delta_type(event_type: &str) -> bool {
    matches!(
        event_type,
        "response.output_text.delta"
            | "response.reasoning_text.delta"
            | "response.reasoning_summary_text.delta"
            | "response.refusal.delta"
            | "response.function_call_arguments.delta"
            | "response.custom_tool_call_input.delta"
            | "response.mcp_call_arguments.delta"
            | "response.code_interpreter_call_code.delta"
    )
}

/// Detects the upstream's silent pre-output abort precisely enough for a safe
/// candidate retry. A missing or non-numeric token count is intentionally not
/// treated as empty: the response then remains owned by the selected route.
pub(in crate::gateway) fn is_empty_responses_incomplete(
    response_payload: &Value,
    saw_output: bool,
    completed_output_items: usize,
) -> bool {
    if response_payload.get("type").and_then(Value::as_str) != Some("response.incomplete")
        || saw_output
        || completed_output_items > 0
    {
        return false;
    }
    if response_payload
        .pointer("/response/output")
        .and_then(Value::as_array)
        .is_some_and(|output_items| !output_items.is_empty())
    {
        return false;
    }
    response_payload
        .pointer("/response/usage/output_tokens")
        .and_then(Value::as_number)
        .is_some_and(|tokens| tokens.to_string() == "0")
}

fn output_item_has_meaningful_tool_call(output_item: &Value) -> bool {
    matches!(
        output_item.get("type").and_then(Value::as_str),
        Some("function_call" | "custom_tool_call" | "mcp_call" | "computer_call")
    ) && ["call_id", "id", "name"].into_iter().any(|field| {
        output_item
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|field_value| !field_value.is_empty())
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
                        .is_some_and(|call_id| !call_id.is_empty())
                        || call.get("function").is_some_and(function_delta_has_output)
                })
            })
}

fn function_delta_has_output(function_delta: &Value) -> bool {
    ["name", "arguments"].into_iter().any(|key| {
        function_delta
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.is_empty())
    })
}

pub(super) fn is_opaque_compaction_event(event_name: &str) -> bool {
    event_name.starts_with("response.compaction.")
}

fn gemini_candidate_has_output_delta(candidate_response: &Value) -> bool {
    candidate_response
        .pointer("/content/parts")
        .and_then(Value::as_array)
        .is_some_and(|parts| {
            parts.iter().any(|part| {
                part.get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
                    || part.get("functionCall").is_some()
                    || part
                        .pointer("/inlineData/data")
                        .and_then(Value::as_str)
                        .is_some_and(|inline_image_data| !inline_image_data.is_empty())
                    || part
                        .pointer("/fileData/fileUri")
                        .and_then(Value::as_str)
                        .is_some_and(|uri| !uri.is_empty())
                    || part
                        .pointer("/executableCode/code")
                        .and_then(Value::as_str)
                        .is_some_and(|code| !code.is_empty())
                    || part
                        .pointer("/codeExecutionResult/output")
                        .and_then(Value::as_str)
                        .is_some_and(|output_text| !output_text.is_empty())
            })
        })
}

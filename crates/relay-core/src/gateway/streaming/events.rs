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
        Value::String(
            preserved
                .error_type
                .as_deref()
                .unwrap_or_else(|| api_error_type(preserved.status, &preserved.code))
                .to_string(),
        ),
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
    pub(in crate::gateway) upstream_error: Option<crate::usage::UpstreamErrorDetails>,
    pub(in crate::gateway) has_data: bool,
    pub(in crate::gateway) valid: bool,
    pub(in crate::gateway) has_output_delta: bool,
    /// True only when this frame carries generated response content.  Context
    /// compaction is intentionally excluded: it is opaque continuation state,
    /// not a user-visible model output.
    pub(in crate::gateway) semantic_output: bool,
    pub(in crate::gateway) is_compaction: bool,
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
    /// The SSE data payload for an opaque Responses compaction event.
    ///
    /// Compaction data is provider-owned and may be encrypted or otherwise
    /// intentionally undecodable. Keep an owned copy so an HTTP-to-WebSocket
    /// bridge can forward it without parsing, normalizing, or dropping it.
    pub(in crate::gateway) raw_data: Option<Vec<u8>>,
}

#[derive(Debug, Eq, PartialEq)]
pub(in crate::gateway) enum TerminalOutcome {
    Success,
    Incomplete,
    Failure,
}

pub(in crate::gateway) fn parse_sse_event(event: &[u8]) -> TerminalEvent {
    let data = crate::protocol::sse_data(event);
    let event_name = crate::protocol::sse_lines(event)
        .filter_map(|line| line.strip_prefix(b"event:"))
        .last()
        .and_then(|value| std::str::from_utf8(value.trim_ascii()).ok());
    if data.is_empty() {
        return TerminalEvent::default();
    }
    if data == b"[DONE]" {
        return TerminalEvent {
            has_data: true,
            valid: true,
            upstream_error: None,
            has_output_delta: false,
            semantic_output: false,
            is_compaction: false,
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
            raw_data: None,
        };
    }
    let value = match serde_json::from_slice::<Value>(&data) {
        Ok(value) => value,
        Err(error) => {
            // Responses context compaction is an opaque provider-owned stream. A
            // few upstream implementations send its delta as a raw/encrypted
            // payload even though ordinary Responses events are JSON. It must be
            // passed through unchanged; rejecting it here turns a valid ongoing
            // compaction into the misleading 502 `stream_invalid` error.
            if event_name.is_some_and(is_opaque_compaction_event) {
                return TerminalEvent {
                    has_data: true,
                    valid: true,
                    is_compaction: true,
                    raw_data: Some(data),
                    ..TerminalEvent::default()
                };
            }
            return TerminalEvent {
                has_data: true,
                upstream_error: Some(super::diagnostics::invalid_event(event, &data, &error)),
                ..TerminalEvent::default()
            };
        }
    };
    let event_type = value.get("type").and_then(Value::as_str);
    let is_compaction = event_name.is_some_and(is_opaque_compaction_event)
        || is_compaction_payload(&value, event_type);
    let upstream_error_category = upstream_event_failure_category(event_type, &value);
    let mut outcome = match event_type {
        Some("response.completed" | "response.done" | "message_stop") => {
            Some(TerminalOutcome::Success)
        }
        Some("response.failed" | "response.cancelled" | "response.canceled" | "error") => {
            Some(TerminalOutcome::Failure)
        }
        Some("response.incomplete") => Some(TerminalOutcome::Incomplete),
        None if upstream_error_category.is_none() && crate::protocol::gemini_incomplete(&value) => {
            Some(TerminalOutcome::Incomplete)
        }
        _ => None,
    };
    let error_category = upstream_error_category.or_else(|| {
        (outcome == Some(TerminalOutcome::Incomplete)).then_some(error_codes::RESPONSE_INCOMPLETE)
    });
    if let Some(category) = error_category {
        let explicitly_incomplete = outcome == Some(TerminalOutcome::Incomplete)
            || matches!(event_type, Some("response.completed" | "response.done"))
                && value.pointer("/response/status").and_then(Value::as_str) == Some("incomplete");
        outcome = Some(
            if category == error_codes::RESPONSE_INCOMPLETE && explicitly_incomplete {
                TerminalOutcome::Incomplete
            } else {
                TerminalOutcome::Failure
            },
        );
    }
    let error_status = error_category.map(|category| {
        let status = upstream_status_from_value(&value)
            .filter(|status| !status.is_success())
            .unwrap_or_else(|| upstream_failure_status(category));
        canonical_upstream_status(status, category)
    });
    let cooldown_hint = rate_limit_body_hint_value(&value, SystemTime::now());
    let preserved_error = preserved_stream_error(&value);
    let has_output_delta = has_output_delta(&value, event_type);
    let semantic_output = has_semantic_output(&value, event_type);
    let usage = find_usage(&value).cloned();
    let applied_service_tier = response_service_tier(&value);
    let response_id = response_id(&value).map(str::to_string);
    let response = value.get("response").cloned();
    let output_item = (value.get("type").and_then(Value::as_str)
        == Some("response.output_item.done"))
    .then(|| value.get("item").cloned())
    .flatten();
    TerminalEvent {
        upstream_error: error_category
            .map(|_| crate::usage::UpstreamErrorDetails::from_value(None, &value)),
        has_data: true,
        valid: true,
        has_output_delta,
        semantic_output,
        is_compaction,
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
        // Preserve the exact provider payload for all compaction forms. The
        // output-item envelope is still useful to SSE clients, but an HTTP to
        // WebSocket bridge must not reserialize the opaque encrypted item.
        raw_data: is_compaction.then_some(data),
    }
}

/// Whether a decoded Responses event contains a compaction item. The item is
/// opaque state that must be forwarded unchanged and does not commit a route
/// before ordinary generation begins.
pub(in crate::gateway) fn is_compaction_payload(value: &Value, event_type: Option<&str>) -> bool {
    matches!(event_type, Some("compaction" | "compaction_summary"))
        || event_type.is_some_and(is_opaque_compaction_event)
        || matches!(
            event_type,
            Some("response.output_item.added" | "response.output_item.done")
        ) && value
            .get("item")
            .and_then(|item| item.get("type"))
            .and_then(Value::as_str)
            .is_some_and(is_compaction_item_type)
}

/// Classifies protocol output without guessing about future event names.
/// Callers that have already forwarded a frame should treat an unknown event
/// conservatively; this function is deliberately limited to confirmed output.
pub(in crate::gateway) fn has_semantic_output(value: &Value, event_type: Option<&str>) -> bool {
    !is_compaction_payload(value, event_type)
        && (has_output_delta(value, event_type) || event_type == Some("response.output_item.done"))
}

/// Known lifecycle and compaction frames are safe to buffer while selecting a
/// route. Anything else is left to the caller's conservative fallback.
pub(in crate::gateway) fn is_known_non_output_event(
    value: &Value,
    event_type: Option<&str>,
) -> bool {
    is_compaction_payload(value, event_type)
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

pub(in crate::gateway) fn has_output_delta(value: &Value, event_type: Option<&str>) -> bool {
    if matches!(
        event_type,
        Some(
            "response.output_text.delta"
                | "response.reasoning_text.delta"
                | "response.reasoning_summary_text.delta"
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
        || value
            .get("candidates")
            .and_then(Value::as_array)
            .is_some_and(|candidates| candidates.iter().any(gemini_candidate_has_output_delta))
}

/// Detects the upstream's silent pre-output abort precisely enough for a safe
/// candidate retry. A missing or non-numeric token count is intentionally not
/// treated as empty: the response then remains owned by the selected route.
pub(in crate::gateway) fn is_empty_responses_incomplete(
    value: &Value,
    saw_output: bool,
    completed_output_items: usize,
) -> bool {
    if value.get("type").and_then(Value::as_str) != Some("response.incomplete")
        || saw_output
        || completed_output_items > 0
    {
        return false;
    }
    if value
        .pointer("/response/output")
        .and_then(Value::as_array)
        .is_some_and(|output| !output.is_empty())
    {
        return false;
    }
    value
        .pointer("/response/usage/output_tokens")
        .and_then(Value::as_number)
        .is_some_and(|tokens| tokens.to_string() == "0")
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

fn is_opaque_compaction_event(event_name: &str) -> bool {
    event_name.starts_with("response.compaction.")
}

fn is_compaction_item_type(item_type: &str) -> bool {
    matches!(item_type, "compaction" | "compaction_summary")
}

fn gemini_candidate_has_output_delta(candidate: &Value) -> bool {
    candidate
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
                        .is_some_and(|data| !data.is_empty())
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
                        .is_some_and(|output| !output.is_empty())
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiline_json_keeps_data_lines_with_all_sse_line_endings() {
        for ending in ["\n", "\r\n", "\r"] {
            let frame = format!("event: response.output_text.delta{ending}data: {{\"type\":{ending}data: \"response.output_text.delta\",{ending}data: \"delta\":\"synthetic\"}}{ending}{ending}");
            let event = parse_sse_event(frame.as_bytes());
            assert!(event.has_data && event.valid && event.semantic_output);
            assert_eq!(event.payload.unwrap()["delta"], "synthetic");
        }
    }

    #[test]
    fn repeated_events_with_mixed_cr_separators_are_not_one_json_payload() {
        let mut bytes = Vec::new();
        for index in 0..5 {
            let ending = if index == 4 { "\n\n" } else { "\n\r" };
            bytes.extend_from_slice(format!("event: response.created\ndata: {{\"type\":\"response.created\",\"sequence_number\":{index}}}{ending}").as_bytes());
        }
        let mut count = 0;
        while let Some(end) = sse_event_end(&bytes) {
            let event = parse_sse_event(&bytes.drain(..end).collect::<Vec<_>>());
            assert!(event.valid);
            assert_eq!(event.payload.unwrap()["sequence_number"], count);
            count += 1;
        }
        assert_eq!(count, 5);
        assert!(bytes.is_empty());
    }

    #[test]
    fn raw_responses_compaction_delta_is_forwarded_without_stream_invalid() {
        let raw = b"encrypted-compaction-fragment";
        let event = parse_sse_event(
            b"event: response.compaction.delta\ndata: encrypted-compaction-fragment\n\n",
        );

        assert!(event.has_data);
        assert!(event.valid);
        assert_eq!(event.outcome, None);
        assert_eq!(event.raw_data.as_deref(), Some(raw.as_slice()));
    }

    #[test]
    fn json_responses_compaction_data_keeps_the_original_bytes() {
        let event = parse_sse_event(
            b"event: response.compaction.delta\ndata: { \"type\": \"response.compaction.delta\", \"opaque\": true }\n\n",
        );

        assert!(event.valid);
        assert_eq!(
            event.raw_data.as_deref(),
            Some(b"{ \"type\": \"response.compaction.delta\", \"opaque\": true }".as_slice())
        );
        assert_eq!(
            event.payload.as_ref().and_then(|value| value.get("opaque")),
            Some(&Value::Bool(true))
        );
    }

    #[test]
    fn compaction_output_items_are_opaque_non_output_events() {
        for item_type in ["compaction", "compaction_summary"] {
            let event = parse_sse_event(
                format!(
                    "event: response.output_item.done\ndata: {{\"type\":\"response.output_item.done\",\"item\":{{\"type\":\"{item_type}\",\"encrypted_content\":\"opaque\"}}}}\n\n"
                )
                .as_bytes(),
            );

            assert!(event.valid);
            assert!(event.is_compaction);
            assert!(!event.semantic_output);
            assert!(event.output_item.is_some());
            assert!(event.raw_data.is_some());
        }
    }

    #[test]
    fn completed_non_compaction_output_item_commits_generation() {
        let event = parse_sse_event(
            b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"content\":[]}}\n\n",
        );

        assert!(event.valid);
        assert!(!event.is_compaction);
        assert!(event.semantic_output);
    }

    #[test]
    fn malformed_non_compaction_event_remains_invalid() {
        let event = parse_sse_event(b"event: response.output_text.delta\ndata: {broken\n\n");

        assert!(event.has_data);
        assert!(!event.valid);
    }

    #[test]
    fn completed_event_cannot_override_an_explicit_noncompleted_response_status() {
        for (status, expected) in [
            ("failed", TerminalOutcome::Failure),
            ("incomplete", TerminalOutcome::Incomplete),
            ("in_progress", TerminalOutcome::Failure),
        ] {
            let data = serde_json::json!({
                "type": "response.completed",
                "response": {"status": status}
            });
            let event = parse_sse_event(format!("data: {data}\n\n").as_bytes());
            assert_eq!(event.outcome, Some(expected), "{status}");
            assert!(event.error_category.is_some(), "{status}");
        }
    }

    #[test]
    fn gemini_filter_and_token_limit_are_incomplete_but_unknown_terminal_is_not() {
        for data in [
            r#"{"promptFeedback":{"blockReason":"SAFETY"},"candidates":[]}"#,
            r#"{"candidates":[{"finishReason":"SAFETY"}]}"#,
            r#"{"candidates":[{"finishReason":"MAX_TOKENS"}]}"#,
        ] {
            let event = parse_sse_event(format!("data: {data}\n\n").as_bytes());
            assert_eq!(event.outcome, Some(TerminalOutcome::Incomplete), "{data}");
            assert_eq!(event.error_category, Some(error_codes::RESPONSE_INCOMPLETE));
        }
        for data in [
            r#"{"candidates":[]}"#,
            r#"{"candidates":[{"finishReason":"NEW_REASON"}]}"#,
            r#"{"promptFeedback":{"blockReason":"NEW_REASON"}}"#,
            r#"{"promptFeedback":{"blockReason":"NEW_REASON"},"candidates":[{"finishReason":"SAFETY"}]}"#,
        ] {
            let event = parse_sse_event(format!("data: {data}\n\n").as_bytes());
            assert_eq!(event.outcome, None, "{data}");
        }
        let with_error = parse_sse_event(
            br#"data: {"error":{"code":500},"candidates":[{"finishReason":"SAFETY"}]}

"#,
        );
        assert_eq!(with_error.outcome, Some(TerminalOutcome::Failure));
    }

    #[test]
    fn native_gemini_media_and_code_parts_are_semantic_output() {
        for part in [
            serde_json::json!({"inlineData":{"mimeType":"image/png","data":"YQ=="}}),
            serde_json::json!({"fileData":{"mimeType":"image/png","fileUri":"gs://example/image"}}),
            serde_json::json!({"executableCode":{"language":"PYTHON","code":"print(1)"}}),
            serde_json::json!({"codeExecutionResult":{"outcome":"OUTCOME_OK","output":"1"}}),
        ] {
            let frame = format!(
                "data: {}\n\n",
                serde_json::json!({"candidates":[{"content":{"parts":[part]}}]})
            );
            assert!(parse_sse_event(frame.as_bytes()).semantic_output);
        }
        let metadata = serde_json::json!({
            "candidates":[{"content":{"parts":[{"thoughtSignature":"opaque"}]},"finishReason":"STOP"}]
        });
        let metadata_only = parse_sse_event(format!("data: {metadata}\n\n").as_bytes());
        assert!(!metadata_only.semantic_output);
    }

    #[test]
    fn empty_incomplete_requires_explicit_zero_tokens_and_no_output() {
        let empty = serde_json::json!({
            "type": "response.incomplete",
            "response": {"output": [], "usage": {"output_tokens": 0}}
        });
        assert!(is_empty_responses_incomplete(&empty, false, 0));

        let decimal_zero = serde_json::json!({
            "type": "response.incomplete",
            "response": {"output": [], "usage": {"output_tokens": 0.0}}
        });
        assert!(!is_empty_responses_incomplete(&decimal_zero, false, 0));
        assert!(!is_empty_responses_incomplete(&empty, true, 0));
        assert!(!is_empty_responses_incomplete(&empty, false, 1));
    }
}

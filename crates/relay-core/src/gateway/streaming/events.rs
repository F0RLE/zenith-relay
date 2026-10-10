use super::*;

mod output;

use output::is_opaque_compaction_event;
pub(in crate::gateway) use output::{
    has_output_delta, has_semantic_output, is_compaction_payload, is_empty_responses_incomplete,
    is_known_non_output_event, is_responses_output_delta_type,
};

pub(in crate::gateway) fn preserved_stream_error(
    event_payload: &Value,
) -> Option<PreservedUpstreamError> {
    let event_type = event_payload.get("type").and_then(Value::as_str);
    let category = upstream_event_failure_category(event_type, event_payload)?;
    let status = upstream_status_from_value(event_payload)
        .filter(|status| !status.is_success())
        .unwrap_or_else(|| upstream_failure_status(category));
    let failure = AttemptFailure::classified_with_hint(
        canonical_upstream_status(status, category),
        category,
        rate_limit_body_hint_value(event_payload, SystemTime::now()),
    );
    preserved_upstream_error_value(&failure, event_payload)
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
        .event_payload
        .as_mut()
        .and_then(|terminal_payload| {
            if terminal_payload.pointer("/response/error").is_some() {
                terminal_payload.pointer_mut("/response/error")
            } else {
                terminal_payload.get_mut("error")
            }
        })
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
    let Some(response_event) = terminal.event_payload else {
        return bytes;
    };
    let event_name = response_event.get("type").and_then(Value::as_str);
    let Ok(encoded_payload) = serde_json::to_vec(&response_event) else {
        return bytes;
    };
    let mut frame = Vec::with_capacity(encoded_payload.len() + event_name.map_or(0, str::len) + 16);
    if let Some(event_name) = event_name {
        frame.extend_from_slice(b"event: ");
        frame.extend_from_slice(event_name.as_bytes());
        frame.push(b'\n');
    }
    frame.extend_from_slice(b"data: ");
    frame.extend_from_slice(&encoded_payload);
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
    pub(in crate::gateway) event_payload: Option<Value>,
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

/// OpenAI names the model it actually served on `model` or `response.model`.
/// Only those fields count: a `degradeN` token anywhere else can be user text.
pub(in crate::gateway) fn served_model_is_degraded(event_payload: &Value) -> bool {
    [
        event_payload.get("model"),
        event_payload.pointer("/response/model"),
    ]
    .into_iter()
    .filter_map(|model| model.and_then(Value::as_str))
    .any(crate::is_degraded_route_model)
}

/// Compare upstream identity, never the public alias or text inside the output.
/// Provider qualification and dated snapshots do not change model identity.
pub(in crate::gateway) fn served_model_is_rejected(event_payload: &Value, expected: &str) -> bool {
    // A provider rejection owns its meaning. In particular, a policy 403
    // must not become a model mismatch that permits account rotation.
    if upstream_event_failure_category(
        event_payload.get("type").and_then(Value::as_str),
        event_payload,
    )
    .is_some()
    {
        return false;
    }
    if served_model_is_degraded(event_payload) {
        return true;
    }
    let expected = model_identity(expected);
    !expected.is_empty()
        && [
            event_payload.get("model"),
            event_payload.pointer("/response/model"),
        ]
        .into_iter()
        .filter_map(|model| model.and_then(Value::as_str))
        .map(model_identity)
        .any(|served| !served.is_empty() && served != expected)
}

fn model_identity(model_id: &str) -> String {
    let model_leaf = model_id
        .trim()
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .trim();
    let suffix_start = [11, 9].into_iter().find_map(|length| {
        let start = model_leaf.len().checked_sub(length)?;
        let suffix = model_leaf.get(start..)?;
        let digits = suffix.strip_prefix('-')?.replace('-', "");
        let valid_shape =
            length == 9 || (suffix.as_bytes()[5] == b'-' && suffix.as_bytes()[8] == b'-');
        (valid_shape && digits.len() == 8 && digits.bytes().all(|byte| byte.is_ascii_digit()))
            .then_some(start)
    });
    model_leaf[..suffix_start.unwrap_or(model_leaf.len())].to_ascii_lowercase()
}

pub(in crate::gateway) fn parse_sse_event(event: &[u8]) -> TerminalEvent {
    let sse_payload = crate::protocol::sse_data(event);
    let event_name = crate::protocol::sse_lines(event)
        .filter_map(|line| line.strip_prefix(b"event:"))
        .last()
        .and_then(|event_name_bytes| std::str::from_utf8(event_name_bytes.trim_ascii()).ok());
    if sse_payload.is_empty() {
        return TerminalEvent::default();
    }
    if sse_payload == b"[DONE]" {
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
            event_payload: None,
            raw_data: None,
        };
    }
    let event_json = match serde_json::from_slice::<Value>(&sse_payload) {
        Ok(parsed_event) => parsed_event,
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
                    raw_data: Some(sse_payload),
                    ..TerminalEvent::default()
                };
            }
            return TerminalEvent {
                has_data: true,
                upstream_error: Some(super::diagnostics::invalid_event(
                    event,
                    &sse_payload,
                    &error,
                )),
                ..TerminalEvent::default()
            };
        }
    };
    let event_type = event_json.get("type").and_then(Value::as_str).or_else(|| {
        event_name.filter(|event_name_value| {
            matches!(
                *event_name_value,
                "error"
                    | "response.failed"
                    | "response.incomplete"
                    | "response.cancelled"
                    | "response.canceled"
                    | "response.completed"
                    | "response.done"
                    | "message_stop"
            )
        })
    });
    let is_compaction = event_name.is_some_and(is_opaque_compaction_event)
        || is_compaction_payload(&event_json, event_type);
    let upstream_error_category = upstream_event_failure_category(event_type, &event_json);
    let mut outcome = match event_type {
        Some("response.completed" | "response.done" | "message_stop") => {
            Some(TerminalOutcome::Success)
        }
        Some("response.failed" | "response.cancelled" | "response.canceled" | "error") => {
            Some(TerminalOutcome::Failure)
        }
        Some("response.incomplete") => Some(TerminalOutcome::Incomplete),
        None if upstream_error_category.is_none()
            && crate::protocol::gemini_incomplete(&event_json) =>
        {
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
                && event_json
                    .pointer("/response/status")
                    .and_then(Value::as_str)
                    == Some("incomplete");
        outcome = Some(
            if category == error_codes::RESPONSE_INCOMPLETE && explicitly_incomplete {
                TerminalOutcome::Incomplete
            } else {
                TerminalOutcome::Failure
            },
        );
    }
    let error_status = error_category.map(|category| {
        let status = upstream_status_from_value(&event_json)
            .filter(|status| !status.is_success())
            .unwrap_or_else(|| upstream_failure_status(category));
        canonical_upstream_status(status, category)
    });
    let cooldown_hint = rate_limit_body_hint_value(&event_json, SystemTime::now());
    let preserved_error = preserved_stream_error(&event_json);
    let has_output_delta = has_output_delta(&event_json, event_type);
    let semantic_output = has_semantic_output(&event_json, event_type);
    let usage = find_usage(&event_json).cloned();
    let applied_service_tier = response_service_tier(&event_json);
    let response_id = response_id(&event_json).map(str::to_string);
    let response_payload = event_json.get("response").cloned();
    let output_item = (event_json.get("type").and_then(Value::as_str)
        == Some("response.output_item.done"))
    .then(|| event_json.get("item").cloned())
    .flatten();
    TerminalEvent {
        upstream_error: error_category
            .map(|_| crate::usage::UpstreamErrorDetails::from_value(None, &event_json)),
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
        response: response_payload,
        output_item,
        event_payload: Some(event_json),
        // Preserve the exact provider payload for all compaction forms. The
        // output-item envelope is still useful to SSE clients, but an HTTP to
        // WebSocket bridge must not reserialize the opaque encrypted item.
        raw_data: is_compaction.then_some(sse_payload),
    }
}

#[cfg(test)]
mod tests;

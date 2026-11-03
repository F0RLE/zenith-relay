use super::*;
use std::sync::atomic::Ordering;
use std::time::{SystemTime, UNIX_EPOCH};

pub(in crate::gateway) fn requested_reasoning_effort(
    request: &Value,
    wire_api: WireApi,
) -> Option<String> {
    let effort = match wire_api {
        WireApi::Responses => request.pointer("/reasoning/effort"),
        WireApi::ChatCompletions => request.get("reasoning_effort"),
        WireApi::Messages => request.pointer("/output_config/effort"),
        WireApi::Gemini => request.pointer("/generationConfig/thinkingConfig/thinkingLevel"),
        WireApi::GeminiInteractions => request.pointer("/generation_config/thinking_level"),
    };
    effort
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|effort| !effort.is_empty() && !effort.eq_ignore_ascii_case("none"))
        .map(crate::catalog::canonical_reasoning_level)
}

pub(in crate::gateway) fn candidate_protocols(wire_api: WireApi) -> &'static [WireApi] {
    // The client's native protocol is first so that Native (passthrough)
    // routes are preferred over bridged routes when both are available.
    // All other protocols follow so a model with only a non-native route
    // is still reachable through the appropriate SourceAdapter bridge.
    match wire_api {
        WireApi::GeminiInteractions => &[WireApi::GeminiInteractions],
        WireApi::Responses => &[
            WireApi::Responses,
            WireApi::Messages,
            WireApi::ChatCompletions,
            WireApi::Gemini,
        ],
        WireApi::ChatCompletions => &[
            WireApi::ChatCompletions,
            WireApi::Responses,
            WireApi::Messages,
            WireApi::Gemini,
        ],
        WireApi::Messages => &[
            WireApi::Messages,
            WireApi::Responses,
            WireApi::ChatCompletions,
            WireApi::Gemini,
        ],
        WireApi::Gemini => &[
            WireApi::Gemini,
            WireApi::Responses,
            WireApi::ChatCompletions,
            WireApi::Messages,
        ],
    }
}

pub(in crate::gateway) fn chat_request_has_supported_content(request_payload: &Value) -> bool {
    let Some(request_fields) = request_payload.as_object() else {
        return false;
    };
    if request_fields.contains_key("audio") {
        return false;
    }
    if let Some(modality_value) = request_fields.get("modalities") {
        let Some(modalities) = modality_value.as_array() else {
            return false;
        };
        if modalities
            .iter()
            .any(|modality| modality.as_str() != Some("text"))
        {
            return false;
        }
    }
    request_fields
        .get("messages")
        .and_then(Value::as_array)
        .is_none_or(|messages| messages.iter().all(chat_message_has_supported_content))
}

fn chat_message_has_supported_content(message_value: &Value) -> bool {
    let Some(message_fields) = message_value.as_object() else {
        return false;
    };
    match message_fields.get("content") {
        None | Some(Value::Null) | Some(Value::String(_)) => true,
        Some(Value::Array(content_parts)) => content_parts.iter().all(|content_part| {
            matches!(
                content_part.get("type").and_then(Value::as_str),
                Some("text" | "image_url" | "file")
            )
        }),
        Some(_) => false,
    }
}

pub(in crate::gateway) fn request_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("relay-{timestamp}-{sequence}")
}

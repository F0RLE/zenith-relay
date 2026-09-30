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
    };
    effort
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|effort| !effort.is_empty() && !effort.eq_ignore_ascii_case("none"))
        .map(str::to_ascii_lowercase)
}

pub(in crate::gateway) fn candidate_protocols(wire_api: WireApi) -> &'static [WireApi] {
    match wire_api {
        WireApi::Responses => &[WireApi::Responses],
        WireApi::ChatCompletions => &[WireApi::ChatCompletions],
        WireApi::Messages => &[WireApi::Messages],
        WireApi::Gemini => &[WireApi::Gemini],
    }
}

pub(in crate::gateway) fn chat_request_is_text_or_image_only(value: &Value) -> bool {
    let Some(request) = value.as_object() else {
        return false;
    };
    if request.contains_key("audio") {
        return false;
    }
    if let Some(modalities) = request.get("modalities") {
        let Some(modalities) = modalities.as_array() else {
            return false;
        };
        if modalities
            .iter()
            .any(|modality| modality.as_str() != Some("text"))
        {
            return false;
        }
    }
    request
        .get("messages")
        .and_then(Value::as_array)
        .is_none_or(|messages| messages.iter().all(chat_message_is_text_or_image_only))
}

fn chat_message_is_text_or_image_only(message: &Value) -> bool {
    let Some(message) = message.as_object() else {
        return false;
    };
    match message.get("content") {
        None | Some(Value::Null) | Some(Value::String(_)) => true,
        Some(Value::Array(parts)) => parts.iter().all(|part| {
            matches!(
                part.get("type").and_then(Value::as_str),
                Some("text" | "image_url")
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

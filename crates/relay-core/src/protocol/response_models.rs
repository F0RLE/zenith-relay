//! Provider-declared execution identities, never model names inside generated text.

use serde_json::Value;

/// Last declaration is the effective serving model; every declaration remains
/// available to the strict route guard, including intermediate fallback hops.
pub(crate) fn declared(payload: &Value) -> impl Iterator<Item = &str> {
    [
        "/interaction/model",
        "/model",
        "/message/model",
        "/response/model",
    ]
    .into_iter()
    .filter_map(|path| payload.pointer(path).and_then(Value::as_str))
    .chain(messages_fallback_models(payload))
}

/// Gemini's reported version remains diagnostics, not a new routing constraint.
pub(crate) fn serving(payload: &Value) -> Option<&str> {
    declared(payload)
        .last()
        .or_else(|| payload.get("modelVersion").and_then(Value::as_str))
}

fn messages_fallback_models(payload: &Value) -> impl Iterator<Item = &str> {
    let messages_event = matches!(
        payload.get("type").and_then(Value::as_str),
        Some("message" | "message_start" | "message_delta" | "content_block_start")
    );
    let blocks = messages_event
        .then_some(payload)
        .into_iter()
        .flat_map(|payload| {
            payload.get("content_block").into_iter().chain(
                [payload.get("content"), payload.pointer("/message/content")]
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_array)
                    .flatten(),
            )
        })
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("fallback"))
        .flat_map(|block| [block.pointer("/from/model"), block.pointer("/to/model")])
        .flatten()
        .filter_map(Value::as_str);
    let iterations = messages_event
        .then_some(payload)
        .into_iter()
        .flat_map(|payload| [payload.get("usage"), payload.pointer("/message/usage")])
        .flatten()
        .filter_map(|usage| usage.get("iterations").and_then(Value::as_array))
        .flatten()
        .filter(|iteration| {
            iteration.get("type").and_then(Value::as_str) == Some("fallback_message")
        })
        .filter_map(|iteration| iteration.get("model").and_then(Value::as_str));
    blocks.chain(iterations)
}

/// A server-side handoff proves execution even before generated text or usage.
pub(crate) fn has_executed_fallback(payload: &Value) -> bool {
    messages_fallback_models(payload).next().is_some()
}

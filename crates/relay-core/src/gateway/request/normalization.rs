use crate::runtime::DefaultServiceTier;
use serde_json::{json, Map, Value};

pub(in crate::gateway) fn request_service_tier(request: &Value) -> DefaultServiceTier {
    if request
        .get("service_tier")
        .and_then(Value::as_str)
        .is_some_and(|tier| {
            tier.eq_ignore_ascii_case("priority") || tier.eq_ignore_ascii_case("fast")
        })
    {
        DefaultServiceTier::Fast
    } else {
        DefaultServiceTier::Standard
    }
}

/// Apply the pool's fast default only when the client did not choose a tier.
///
/// `priority` is the upstream OpenAI spelling. Standard deliberately remains
/// implicit, matching the Codex/Cockpit behavior and preserving arbitrary
/// client-owned values such as `flex`.
pub(in crate::gateway) fn apply_default_service_tier_if_missing(
    request: &mut Value,
    default: DefaultServiceTier,
) {
    if default != DefaultServiceTier::Fast {
        return;
    }
    let Some(object) = request.as_object_mut() else {
        return;
    };
    if object.contains_key("service_tier") {
        return;
    }
    object.insert(
        "service_tier".to_string(),
        Value::String("priority".to_string()),
    );
}

pub(in crate::gateway) fn normalize_account_request(
    object: &mut Map<String, Value>,
    responses_lite: bool,
) {
    // Native account settings are opaque client selections. In particular,
    // never translate or filter `service_tier`, `reasoning.effort`, or
    // `reasoning.summary` according to Relay pool policy. Responses Lite is
    // the sole exception: its upstream contract requires `context=all_turns`.
    object.insert("store".to_string(), Value::Bool(false));
    object.insert("stream".to_string(), Value::Bool(true));
    object.remove("max_output_tokens");
    sanitize_unstored_reasoning_items(object);
    if responses_lite {
        // Codex Responses Lite accepts only complete reasoning history. Keep
        // the client-selected effort and summary settings, but always supply
        // the mandatory context mode before the request reaches either the
        // HTTP or WebSocket account transport.
        let reasoning = object
            .entry("reasoning".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        if !reasoning.is_object() {
            *reasoning = Value::Object(Map::new());
        }
        reasoning
            .as_object_mut()
            .expect("reasoning was normalized to an object")
            .insert(
                "context".to_string(),
                Value::String("all_turns".to_string()),
            );
        // Responses Lite has a stricter tool contract than the regular
        // Responses endpoint.  The upstream currently requires an explicit
        // boolean and only supports serial tool execution, even when the
        // client omitted the field.  Keep the client-owned tool definitions
        // untouched, but pin this transport-level switch to false.  This is
        // deliberately done for both OAuth and compact Lite routes so HTTP
        // and WebSocket requests cannot diverge.
        if !matches!(object.get("parallel_tool_calls"), Some(Value::Bool(false))) {
            object.insert("parallel_tool_calls".to_string(), Value::Bool(false));
        }
    }
    match object.get("input") {
        Some(Value::String(text)) if text.trim().is_empty() => {
            object.insert("input".to_string(), Value::Array(Vec::new()));
        }
        Some(Value::String(text)) => {
            object.insert(
                "input".to_string(),
                json!([{"role": "user", "content": [{"type": "input_text", "text": text}]}]),
            );
        }
        Some(Value::Object(item)) => {
            object.insert(
                "input".to_string(),
                Value::Array(vec![Value::Object(item.clone())]),
            );
        }
        _ => {}
    }
}

pub(in crate::gateway) fn responses_lite_parallel_tool_calls_valid(
    object: &Map<String, Value>,
) -> bool {
    object
        .get("parallel_tool_calls")
        .is_none_or(Value::is_boolean)
}

fn sanitize_unstored_reasoning_items(object: &mut Map<String, Value>) {
    let Some(input) = object.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    for item in input {
        let Some(item) = item.as_object_mut() else {
            continue;
        };
        if item.get("type").and_then(Value::as_str) != Some("reasoning") {
            continue;
        }
        let has_encrypted_content = item
            .get("encrypted_content")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        if !has_encrypted_content {
            item.remove("id");
            item.remove("encrypted_content");
        }
    }
}

pub(in crate::gateway) fn try_recover_encrypted_content(
    request: &mut Value,
    attempted: &mut bool,
) -> bool {
    if *attempted {
        return false;
    }
    let mut recovered = request.clone();
    let mut changed = false;
    strip_encrypted_reasoning(&mut recovered, &mut changed);
    if !changed {
        return false;
    }
    *request = recovered;
    *attempted = true;
    true
}

fn strip_encrypted_reasoning(value: &mut Value, changed: &mut bool) {
    match value {
        Value::Array(values) => {
            values.retain_mut(|value| {
                if is_encrypted_compaction(value) {
                    *changed = true;
                    return false;
                }
                strip_encrypted_reasoning(value, changed);
                true
            });
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("reasoning")
                && object
                    .get("encrypted_content")
                    .and_then(Value::as_str)
                    .is_some_and(|content| !content.trim().is_empty())
            {
                object.remove("encrypted_content");
                object.remove("id");
                *changed = true;
            }
            for value in object.values_mut() {
                strip_encrypted_reasoning(value, changed);
            }
        }
        _ => {}
    }
}

fn is_encrypted_compaction(value: &Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    matches!(
        object.get("type").and_then(Value::as_str),
        Some("compaction" | "compaction_summary")
    ) && object
        .get("encrypted_content")
        .and_then(Value::as_str)
        .is_some_and(|content| !content.trim().is_empty())
}

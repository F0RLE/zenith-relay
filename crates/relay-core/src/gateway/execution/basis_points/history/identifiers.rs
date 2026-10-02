use super::has_item_ciphertext;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Responses rejects `input[].id` and `call_id` above this length.
const RESPONSES_ID_LIMIT: usize = 64;

/// Keep a Responses item identifier within the upstream limit.
///
/// A known namespace prefix stays intact. The remainder is a stable hash of
/// the original value, so a historic call and its output still match after a
/// model switch replaces a long foreign identifier.
pub(in crate::gateway::execution::basis_points) fn fit_responses_id(id: &str) -> String {
    if id.chars().count() <= RESPONSES_ID_LIMIT {
        return id.to_string();
    }
    // Longer prefixes first so `ctc_` is not mistaken for a shorter token.
    let prefix = [
        "ctc_", "call_", "cmp_", "mcp_", "msg_", "fc_", "fs_", "rs_", "ws_", "ci_", "cu_", "ig_",
    ]
    .into_iter()
    .find(|prefix| id.starts_with(prefix))
    .unwrap_or("");
    let digest = hex::encode(Sha256::digest(id.as_bytes()));
    let take = (RESPONSES_ID_LIMIT - prefix.chars().count()).min(32);
    let mut fitted = String::with_capacity(prefix.len() + take);
    fitted.push_str(prefix);
    fitted.extend(digest.chars().take(take));
    fitted
}

pub(in crate::gateway::execution::basis_points) fn limit_responses_identifiers(
    items: &mut [Value],
) {
    for item in items {
        let Some(object) = item.as_object_mut() else {
            continue;
        };
        shrink_field(object, "call_id", "");
        if has_item_ciphertext(object.get("encrypted_content")) {
            continue;
        }
        let kind = object.get("type").and_then(Value::as_str).unwrap_or("");
        let prefix = item_id_prefix(kind);
        shrink_field(object, "id", prefix);
    }
}

fn shrink_field(object: &mut serde_json::Map<String, Value>, field: &str, required_prefix: &str) {
    let Some(current) = object.get(field).and_then(Value::as_str).map(str::to_owned) else {
        return;
    };
    let normalized = if required_prefix.is_empty() || current.starts_with(required_prefix) {
        current.clone()
    } else if current.chars().count() > RESPONSES_ID_LIMIT || required_prefix == "ws_" {
        format!("{required_prefix}{current}")
    } else {
        return;
    };
    let fitted = fit_responses_id(&normalized);
    if fitted != current {
        object.insert(field.to_string(), Value::String(fitted));
    }
}

fn item_id_prefix(kind: &str) -> &'static str {
    match kind {
        "web_search_call" => "ws_",
        "function_call" | "function_call_output" => "fc_",
        "custom_tool_call" | "custom_tool_call_output" => "ctc_",
        _ => "",
    }
}

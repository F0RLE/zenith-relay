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
pub(in crate::gateway::execution::basis_points) fn fit_responses_id(identifier: &str) -> String {
    if identifier.chars().count() <= RESPONSES_ID_LIMIT {
        return identifier.to_string();
    }
    // Longer prefixes first so `ctc_` is not mistaken for a shorter token.
    let prefix = [
        "ctc_", "call_", "cmp_", "mcp_", "msg_", "fc_", "fs_", "rs_", "ws_", "ci_", "cu_", "ig_",
    ]
    .into_iter()
    .find(|prefix| identifier.starts_with(prefix))
    .unwrap_or("");
    let digest = hex::encode(Sha256::digest(identifier.as_bytes()));
    let take = (RESPONSES_ID_LIMIT - prefix.chars().count()).min(32);
    let mut fitted = String::with_capacity(prefix.len() + take);
    fitted.push_str(prefix);
    fitted.extend(digest.chars().take(take));
    fitted
}

pub(in crate::gateway::execution::basis_points) fn limit_responses_identifiers(
    input_items: &mut [Value],
) {
    for input_item in input_items {
        let Some(input_object) = input_item.as_object_mut() else {
            continue;
        };
        shrink_field(input_object, "call_id", "");
        if has_item_ciphertext(input_object.get("encrypted_content")) {
            continue;
        }
        let kind = input_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("");
        let prefix = item_id_prefix(kind);
        shrink_field(input_object, "id", prefix);
    }
}

fn shrink_field(
    object_fields: &mut serde_json::Map<String, Value>,
    field: &str,
    required_prefix: &str,
) {
    let Some(existing_identifier) = object_fields
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return;
    };
    let normalized = if required_prefix.is_empty()
        || existing_identifier.starts_with(required_prefix)
    {
        existing_identifier.clone()
    } else if existing_identifier.chars().count() > RESPONSES_ID_LIMIT || required_prefix == "ws_" {
        format!("{required_prefix}{existing_identifier}")
    } else {
        return;
    };
    let fitted = fit_responses_id(&normalized);
    if fitted != existing_identifier {
        object_fields.insert(field.to_string(), Value::String(fitted));
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

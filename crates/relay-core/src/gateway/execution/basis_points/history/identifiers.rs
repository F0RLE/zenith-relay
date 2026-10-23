use super::has_item_ciphertext;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Responses rejects `input[].id` and `call_id` above this length.
const RESPONSES_ID_LIMIT: usize = 64;

/// Keep a Responses item identifier within the upstream length and character limits.
///
/// A known namespace prefix stays intact. The remainder is a stable hash of
/// the original value, so a historic call and its output still match after a
/// model switch replaces a foreign identifier.
pub(in crate::gateway::execution::basis_points) fn fit_responses_id(identifier: &str) -> String {
    if valid_responses_item_id(identifier) {
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

fn valid_responses_item_id(identifier: &str) -> bool {
    identifier.len() <= RESPONSES_ID_LIMIT
        && identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
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
    // Short call IDs are opaque associations, not Responses item identifiers.
    if field == "call_id" && existing_identifier.chars().count() <= RESPONSES_ID_LIMIT {
        return;
    }
    let normalized =
        if required_prefix.is_empty() || existing_identifier.starts_with(required_prefix) {
            existing_identifier.clone()
        } else if !valid_responses_item_id(&existing_identifier) || required_prefix == "ws_" {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn short_foreign_item_ids_fit_the_responses_character_set() {
        for (identifier, prefix) in [
            ("fc_call_rb1.3.0.synthetic", "fc_"),
            ("ctc_tool.0", "ctc_"),
            ("msg_message/1", "msg_"),
            ("ws_search+1", "ws_"),
            ("fc_инструмент", "fc_"),
        ] {
            let fitted = fit_responses_id(identifier);
            assert_ne!(fitted, identifier);
            assert!(fitted.starts_with(prefix));
            assert!(fitted.len() <= RESPONSES_ID_LIMIT);
            assert!(fitted
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')));
            assert_eq!(fit_responses_id(identifier), fitted);
            assert_eq!(fit_responses_id(&fitted), fitted);
        }
        for identifier in ["fc_call_1-2", "ctc_tool_1", "msg_message_1"] {
            assert_eq!(fit_responses_id(identifier), identifier);
        }
        assert_ne!(fit_responses_id("fc_call.1"), fit_responses_id("fc_call_1"));
        assert_ne!(fit_responses_id("fc_call.1"), fit_responses_id("fc_call+1"));
    }

    #[test]
    fn item_id_normalization_preserves_tool_pairs_and_ciphertext() {
        let call_id = "call_rb1.3.0.synthetic";
        let mut items = vec![
            json!({"type":"function_call", "id":format!("fc_{call_id}"), "call_id":call_id}),
            json!({"type":"function_call_output", "id":format!("fc_{call_id}"), "call_id":call_id, "output":"ok"}),
            json!({"type":"reasoning", "id":"rs_opaque.native", "encrypted_content":"synthetic_ciphertext"}),
        ];
        let reasoning = items[2].clone();
        limit_responses_identifiers(&mut items);
        let item_id = items[0]["id"].as_str().unwrap();
        assert!(!item_id.contains('.'));
        assert_eq!(items[1]["id"], item_id);
        assert_eq!(items[0]["call_id"], call_id);
        assert_eq!(items[1]["call_id"], items[0]["call_id"]);
        assert_eq!(items[2], reasoning);
        let fitted = items.clone();
        limit_responses_identifiers(&mut items);
        assert_eq!(items, fitted);
    }

    #[test]
    fn invalid_foreign_item_ids_gain_the_required_namespace() {
        let mut items = vec![
            json!({"type":"function_call", "id":"tool.foreign", "call_id":"call_native"}),
            json!({"type":"custom_tool_call", "id":"tool.foreign", "call_id":"call_custom"}),
            json!({"type":"function_call", "id":"call_native-valid", "call_id":"call_valid"}),
        ];
        limit_responses_identifiers(&mut items);
        assert!(items[0]["id"].as_str().unwrap().starts_with("fc_"));
        assert!(items[1]["id"].as_str().unwrap().starts_with("ctc_"));
        for item in &items[..2] {
            assert!(!item["id"].as_str().unwrap().contains('.'));
        }
        assert_eq!(items[2]["id"], "call_native-valid");
    }
}

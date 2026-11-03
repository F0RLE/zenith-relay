use super::has_item_ciphertext;
use crate::protocol::AdapterError;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::borrow::Cow;

/// Responses rejects `input[].id` and `call_id` above this length.
const RESPONSES_ID_LIMIT: usize = 64;

/// Short call IDs are opaque; only overlong IDs acquire the replay hash.
pub(in crate::gateway::execution::basis_points) fn replay_call_id(id: &str) -> Cow<'_, str> {
    if id.len() <= RESPONSES_ID_LIMIT
        || id.chars().take(RESPONSES_ID_LIMIT + 1).count() <= RESPONSES_ID_LIMIT
    {
        Cow::Borrowed(id)
    } else {
        Cow::Owned(fit_responses_id(id))
    }
}

/// Reserve the identities visible upstream, including collapsed batch parents.
pub(in crate::gateway::execution::basis_points) fn replay_call_ids(
    request: &Value,
) -> impl Iterator<Item = Cow<'_, str>> {
    request
        .get("input")
        .into_iter()
        .flat_map(|input| {
            input
                .as_array()
                .map_or(std::slice::from_ref(input), Vec::as_slice)
        })
        .filter_map(|item| item.get("call_id").and_then(Value::as_str))
        .flat_map(|id| {
            std::iter::once(replay_call_id(id))
                .chain(super::super::batch::parent_call_id(id).map(Cow::Owned))
        })
}

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
) -> Result<(), AdapterError> {
    check_call_id_collisions(input_items)?;
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
    Ok(())
}

/// Check before mutation so different source identities never become one pair.
fn check_call_id_collisions(input_items: &[Value]) -> Result<(), AdapterError> {
    let identifiers = || {
        input_items
            .iter()
            .filter_map(|item| item.get("call_id").and_then(Value::as_str))
    };
    let mut rewritten = std::collections::HashMap::new();
    for id in identifiers() {
        if let Cow::Owned(fitted) = replay_call_id(id)
            && let Some(previous) = rewritten.insert(fitted, id)
            && previous != id
        {
            return Err(AdapterError::invalid_request().with_parameter("input.call_id"));
        }
    }
    // Ordinary short-ID history needs no map allocation or second scan.
    if !rewritten.is_empty() && identifiers().any(|id| rewritten.contains_key(id)) {
        return Err(AdapterError::invalid_request().with_parameter("input.call_id"));
    }
    Ok(())
}

fn shrink_field(
    object_fields: &mut serde_json::Map<String, Value>,
    field: &str,
    required_prefix: &str,
) {
    let Some(existing_identifier) = object_fields.get(field).and_then(Value::as_str) else {
        return;
    };
    // Short call IDs are opaque associations, not Responses item identifiers.
    if field == "call_id" {
        if let Cow::Owned(fitted) = replay_call_id(existing_identifier) {
            object_fields.insert(field.to_string(), Value::String(fitted));
        }
        return;
    }
    let normalized =
        if required_prefix.is_empty() || existing_identifier.starts_with(required_prefix) {
            Cow::Borrowed(existing_identifier)
        } else if !valid_responses_item_id(existing_identifier) || required_prefix == "ws_" {
            Cow::Owned(format!("{required_prefix}{existing_identifier}"))
        } else {
            return;
        };
    let fitted = if valid_responses_item_id(&normalized) {
        normalized
    } else {
        Cow::Owned(fit_responses_id(&normalized))
    };
    if let Cow::Owned(fitted) = fitted {
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
    fn identifier_collision_errors_leave_history_untouched() {
        let long = format!("call_{}", "synthetic".repeat(16));
        let fitted = replay_call_id(&long).into_owned();
        for ids in [[&long, &fitted], [&fitted, &long]] {
            let mut items = vec![
                json!({"type":"function_call","id":"fc_invalid.id","call_id":ids[0]}),
                json!({"type":"function_call_output","call_id":ids[1],"output":"Synthetic result"}),
            ];
            let before = items.clone();
            let pointer = items[0]["call_id"].as_str().unwrap().as_ptr();
            let error = limit_responses_identifiers(&mut items).unwrap_err();
            assert_eq!(error.parameter(), Some("input.call_id"));
            assert_eq!(items, before);
            assert_eq!(items[0]["call_id"].as_str().unwrap().as_ptr(), pointer);
        }
    }

    #[test]
    fn replay_call_identity_preserves_short_opaque_ids_and_matches_wire_normalization() {
        for id in [
            " call.with.padding ".to_owned(),
            "😀".repeat(64),
            "call_synthetic".into(),
        ] {
            assert!(matches!(replay_call_id(&id), Cow::Borrowed(_)));
            assert_eq!(replay_call_id(&id), id);
        }
        for id in ["x".repeat(65), "😀".repeat(65)] {
            let expected = replay_call_id(&id);
            assert!(matches!(expected, Cow::Owned(_)));
            let mut items = vec![json!({"type":"function_call","call_id":id})];
            limit_responses_identifiers(&mut items).unwrap();
            assert_eq!(items[0]["call_id"].as_str().unwrap(), expected);
            assert_eq!(replay_call_id(&expected), expected);
        }
    }

    mod benchmark;

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
            assert!(
                fitted
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            );
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
        limit_responses_identifiers(&mut items).unwrap();
        let item_id = items[0]["id"].as_str().unwrap();
        assert!(!item_id.contains('.'));
        assert_eq!(items[1]["id"], item_id);
        assert_eq!(items[0]["call_id"], call_id);
        assert_eq!(items[1]["call_id"], items[0]["call_id"]);
        assert_eq!(items[2], reasoning);
        let fitted = items.clone();
        limit_responses_identifiers(&mut items).unwrap();
        assert_eq!(items, fitted);
    }

    #[test]
    fn invalid_foreign_item_ids_gain_the_required_namespace() {
        let mut items = vec![
            json!({"type":"function_call", "id":"tool.foreign", "call_id":"call_native"}),
            json!({"type":"custom_tool_call", "id":"tool.foreign", "call_id":"call_custom"}),
            json!({"type":"function_call", "id":"call_native-valid", "call_id":"call_valid"}),
        ];
        limit_responses_identifiers(&mut items).unwrap();
        assert!(items[0]["id"].as_str().unwrap().starts_with("fc_"));
        assert!(items[1]["id"].as_str().unwrap().starts_with("ctc_"));
        for item in &items[..2] {
            assert!(!item["id"].as_str().unwrap().contains('.'));
        }
        assert_eq!(items[2]["id"], "call_native-valid");
    }
}

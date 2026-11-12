use serde_json::{Map, Value, json};

/// Observed Codex Desktop page snapshots are context, not executed tool results.
/// Require their exact envelope and unambiguous JSON before changing item kinds.
pub(in crate::gateway) fn normalize_codex_page_context(
    request: &mut Map<String, Value>,
    original: &[u8],
) {
    let Some(items) = request.get_mut("input").and_then(Value::as_array_mut) else {
        return;
    };
    if !items.iter().any(is_page_context)
        || std::str::from_utf8(original)
            .ok()
            .is_none_or(|text| crate::strict_json::validate(text).is_err())
    {
        return;
    }
    for item in items {
        if is_page_context(item) {
            let text = item["output"].take();
            *item = json!({
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": text}],
            });
        }
    }
}

fn is_page_context(item: &Value) -> bool {
    let Some(fields) = item.as_object() else {
        return false;
    };
    if fields.len() != 4
        || item["type"] != "function_call_output"
        || item["namespace"] != "agent"
        || !fields
            .keys()
            .all(|key| matches!(key.as_str(), "type" | "namespace" | "name" | "output"))
    {
        return false;
    }
    let (Some(name), Some(text)) = (item["name"].as_str(), item["output"].as_str()) else {
        return false;
    };
    if name == "codex_apps_open_page" {
        return crate::strict_json::parse(text).is_ok_and(|page| {
            page.is_object()
                && page.get("page_id").is_some_and(|id| {
                    id.is_null() || id.as_str().is_some_and(|value| !value.trim().is_empty())
                })
        });
    }
    let Some((part, total)) = name
        .strip_prefix("codex_apps_selected_page_part_")
        .and_then(|suffix| suffix.split_once("_of_"))
    else {
        return false;
    };
    let canonical_number = |value: &str| {
        value
            .parse::<usize>()
            .ok()
            .filter(|number| *number > 0 && number.to_string() == value)
    };
    let (Some(part_number), Some(total_number)) = (canonical_number(part), canonical_number(total))
    else {
        return false;
    };
    part_number <= total_number
        && text
            .strip_prefix(&format!(
                "Companion Page snapshot part {part} of {total}:\n"
            ))
            .is_some_and(|snapshot| !snapshot.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(name: &str, output: &str) -> Value {
        json!({"type":"function_call_output", "namespace":"agent", "name":name, "output":output})
    }

    fn normalize(mut request: Value) -> Value {
        let original = serde_json::to_vec(&request).unwrap();
        normalize_codex_page_context(request.as_object_mut().unwrap(), &original);
        request
    }

    #[test]
    fn page_context_preserves_text_order_and_real_tool_history() {
        let text = r#"{"page_id":"synthetic-page","document_view":null}"#;
        let call = json!({"type":"function_call", "call_id":"call_keep", "name":"lookup", "arguments":"{}"});
        let result = json!({"type":"function_call_output", "call_id":"call_keep", "output":"synthetic result"});
        let original = json!({"model":"synthetic", "input":[
            {"role":"user", "content":"Inspect the page"},
            context("codex_apps_open_page", text), call, result,
        ], "metadata":{"keep":"unchanged"}});
        let normalized = normalize(original.clone());
        assert_eq!(
            normalized["input"][1],
            json!({"type":"message", "role":"user", "content":[{"type":"input_text", "text":text}]})
        );
        for index in [0, 2, 3] {
            assert_eq!(normalized["input"][index], original["input"][index]);
        }
        assert_eq!(normalized["metadata"], original["metadata"]);
        assert_eq!(normalize(normalized.clone()), normalized);
        assert_eq!(
            normalize(json!({"input":[context("codex_apps_open_page", r#"{"page_id":null}"#)]}))["input"]
                [0]["role"],
            "user"
        );
    }

    #[test]
    fn selected_snapshot_parts_preserve_incomplete_json_as_context() {
        let text =
            "Companion Page snapshot part 1 of 2:\n{\"page_id\":\"synthetic\",\"document_view\":";
        let normalized =
            normalize(json!({"input":[context("codex_apps_selected_page_part_1_of_2", text)]}));
        assert_eq!(normalized["input"][0]["content"][0]["text"], text);
    }

    #[test]
    fn tool_identities_extensions_and_unrecognized_context_are_unchanged() {
        let valid = context("codex_apps_open_page", r#"{"page_id":null}"#);
        let mut invalid = Vec::new();
        for field in ["call_id", "tool_call_id", "id", "status", "extension"] {
            for value in [Value::Null, json!(""), json!("call_keep"), json!(false)] {
                let mut item = valid.clone();
                item[field] = value;
                invalid.push(item);
            }
        }
        for output in [
            "{}",
            "[]",
            r#"{"page_id":""}"#,
            r#"{"page_id":42}"#,
            r#"{"page_id":null,"page_id":"synthetic"}"#,
        ] {
            invalid.push(context("codex_apps_open_page", output));
        }
        for name in [
            "lookup",
            "codex_apps_selected_page_part_0_of_1",
            "codex_apps_selected_page_part_2_of_1",
            "codex_apps_selected_page_part_01_of_2",
            "codex_apps_selected_page_part_1_of_02",
        ] {
            invalid.push(context(name, "Companion Page snapshot part 1 of 2:\n{}"));
        }
        let mut wrong_namespace = valid;
        wrong_namespace["namespace"] = json!("other");
        invalid.push(wrong_namespace);
        let original = json!({"input":invalid});
        assert_eq!(normalize(original.clone()), original);
    }

    #[test]
    fn ambiguous_raw_envelopes_are_not_reinterpreted() {
        let raw = r#"{"input":[{"type":"message","type":"function_call_output","namespace":"agent","name":"codex_apps_open_page","output":"{\"page_id\":null}"}]}"#;
        let mut request: Value = serde_json::from_str(raw).unwrap();
        let original = request.clone();
        normalize_codex_page_context(request.as_object_mut().unwrap(), raw.as_bytes());
        assert_eq!(request, original);
    }
}

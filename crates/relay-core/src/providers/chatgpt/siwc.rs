//! SIWC preview contract: preserve options or reject before network dispatch.
use crate::protocol::AdapterError;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(crate) fn sanitize_siwc_headers(headers: &mut reqwest::header::HeaderMap) {
    let removed: Vec<_> = headers
        .keys()
        .filter(|name| {
            !matches!(
                name.as_str(),
                "authorization"
                    | "content-type"
                    | "accept"
                    | "user-agent"
                    | "accept-encoding"
                    | "content-length"
            )
        })
        .cloned()
        .collect();
    for name in removed {
        headers.remove(name);
    }
}

pub(crate) fn prepare_siwc_request(body: &mut Value) -> Result<(), AdapterError> {
    let object = body
        .as_object_mut()
        .ok_or_else(|| AdapterError::parameter_unsupported_for("body"))?;
    for field in [
        "background",
        "conversation",
        "max_output_tokens",
        "max_tool_calls",
        "metadata",
        "moderation",
        "multi_agent",
        "prompt",
        "prompt_cache_retention",
        "safety_identifier",
        "temperature",
        "top_logprobs",
        "top_p",
        "truncation",
        "user",
    ] {
        if object.contains_key(field) {
            return Err(AdapterError::parameter_unsupported_for(field));
        }
    }
    if object
        .get("previous_response_id")
        .is_some_and(|v| !v.is_null())
    {
        return Err(AdapterError::parameter_unsupported_for(
            "previous_response_id",
        ));
    }
    if object
        .get("store")
        .is_some_and(|v| v != &Value::Bool(false))
    {
        return Err(AdapterError::parameter_unsupported_for("store"));
    }
    if let Some(tools) = object.get("tools") {
        validate_tools(tools)?;
    }
    let input = object.entry("input").or_insert_with(|| json!([]));
    if let Some(text) = input.as_str() {
        *input = json!([{"role":"user","content":text}]);
    }
    let items = input
        .as_array_mut()
        .ok_or_else(|| AdapterError::parameter_unsupported_for("input"))?;
    for item in items.iter() {
        if item.get("role").and_then(Value::as_str) == Some("system") {
            return Err(AdapterError::parameter_unsupported_for("input.role.system"));
        }
        if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
            validate_tools(
                item.get("tools")
                    .ok_or_else(|| AdapterError::parameter_unsupported_for("additional_tools"))?,
            )?;
        }
        if let Some(content) = item.get("content").and_then(Value::as_array) {
            for part in content {
                if matches!(
                    part.get("type").and_then(Value::as_str),
                    Some("input_audio" | "input_video" | "audio" | "video")
                ) {
                    return Err(AdapterError::parameter_unsupported_for("input.media"));
                }
            }
        }
    }
    let has_tool_history = items
        .iter()
        .any(|item| item.get("type").and_then(Value::as_str) == Some("additional_tools"));
    let historical_tools: BTreeMap<_, _> = items
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("additional_tools"))
        .filter_map(|item| item.get("tools").and_then(Value::as_array))
        .flatten()
        .filter(|tool| {
            matches!(
                tool.get("type").and_then(Value::as_str),
                Some("function" | "custom")
            )
        })
        .filter_map(|tool| {
            tool.get("name")
                .and_then(Value::as_str)
                .map(|name| (name.to_string(), tool.to_string()))
        })
        .collect();
    // Preserve declared history; current definitions override it only at the tail.
    let mut bare = Vec::new();
    if let Some(tools) = object.get_mut("tools").and_then(Value::as_array_mut) {
        tools.retain(|tool| {
            if matches!(
                tool.get("type").and_then(Value::as_str),
                Some("function" | "custom")
            ) {
                if tool
                    .get("name")
                    .and_then(Value::as_str)
                    .and_then(|name| historical_tools.get(name))
                    .is_none_or(|previous| previous != &tool.to_string())
                {
                    bare.push(tool.clone());
                }
                false
            } else {
                true
            }
        });
    }
    if !bare.is_empty() {
        let input = object
            .get_mut("input")
            .and_then(Value::as_array_mut)
            .expect("validated input");
        let (prefix, appended): (Vec<_>, Vec<_>) = bare.into_iter().partition(|tool| {
            !has_tool_history
                || tool
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| {
                        !historical_tools.contains_key(name)
                            && input.iter().any(|item| {
                                matches!(
                                    item.get("type").and_then(Value::as_str),
                                    Some("function_call" | "custom_tool_call")
                                ) && item.get("name").and_then(Value::as_str) == Some(name)
                                    && item.get("namespace").is_none_or(Value::is_null)
                            })
                    })
        });
        // A legacy call without loading history still needs its definition first.
        if !prefix.is_empty() {
            input.insert(
                0,
                json!({"type":"additional_tools","role":"developer","tools":prefix}),
            );
        }
        if !appended.is_empty() {
            input.push(json!({"type":"additional_tools","role":"developer","tools":appended}));
        }
    }
    object.remove("previous_response_id");
    object.insert("store".into(), Value::Bool(false));
    object.insert("stream".into(), Value::Bool(true));
    Ok(())
}

fn validate_tools(tools: &Value) -> Result<(), AdapterError> {
    for tool in tools
        .as_array()
        .ok_or_else(|| AdapterError::parameter_unsupported_for("tools"))?
    {
        match tool.get("type").and_then(Value::as_str) {
            Some("function" | "custom" | "web_search" | "web_search_preview") => {}
            Some("namespace") => {
                for child in tool
                    .get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(|| AdapterError::parameter_unsupported_for("tools.namespace"))?
                {
                    if !matches!(
                        child.get("type").and_then(Value::as_str),
                        Some("function" | "custom")
                    ) {
                        return Err(AdapterError::parameter_unsupported_for("tools.namespace"));
                    }
                }
            }
            _ => return Err(AdapterError::parameter_unsupported_for("tools")),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sends_bearer_without_codex_workspace_or_session_headers() {
        let mut headers = reqwest::header::HeaderMap::new();
        for name in [
            "authorization",
            "content-type",
            "chatgpt-account-id",
            "x-codex-turn-state",
            "cookie",
            "openai-beta",
            "originator",
        ] {
            headers.insert(name, "synthetic".parse().unwrap());
        }
        sanitize_siwc_headers(&mut headers);
        assert_eq!(headers.len(), 2);
        assert!(headers.contains_key("authorization"));
    }
    #[test]
    fn preserves_call_result_history_and_bare_tool_identity() {
        let mut request = json!({"model":"synthetic", "input":[{"type":"function_call","name":"lookup","call_id":"call_1","arguments":"{}"},{"type":"function_call_output","call_id":"call_1","output":"ok"}], "tools":[{"type":"function","name":"lookup","parameters":{"type":"object","additionalProperties":false}}], "stream":false});
        let history = request["input"].clone();
        let tool = request["tools"][0].clone();
        prepare_siwc_request(&mut request).unwrap();
        assert_eq!(
            request["input"][0],
            json!({"type":"additional_tools","role":"developer","tools":[tool]})
        );
        assert_eq!(request["input"][1], history[0]);
        assert_eq!(request["input"][2], history[1]);
        assert_eq!(request["stream"], true);
        assert_eq!(request["store"], false);
    }
    #[test]
    fn preserves_custom_and_namespaced_tools_without_duplicate_insertion() {
        let tool = json!({"type":"custom","name":"edit","format":{"type":"text"}});
        let namespace = json!({"type":"namespace","name":"files","tools":[{
            "type":"function","name":"read","parameters":{"type":"object"}
        }]});
        let history = json!([
            {"type":"custom_tool_call","name":"edit","call_id":"call_edit","input":"Synthetic patch"},
            {"type":"custom_tool_call_output","call_id":"call_edit","output":"Synthetic result"}
        ]);
        let choice = json!({"type":"custom","name":"edit"});
        let mut request = json!({"input":history,"tools":[tool,namespace],"tool_choice":choice});
        prepare_siwc_request(&mut request).unwrap();
        assert_eq!(
            request["input"],
            json!([
                {"type":"additional_tools","role":"developer","tools":[tool]},
                history[0],history[1]
            ])
        );
        assert_eq!(request["tools"], json!([namespace]));
        assert_eq!(request["tool_choice"], choice);
        let prepared = request.clone();
        prepare_siwc_request(&mut request).unwrap();
        assert_eq!(request, prepared);
    }
    #[test]
    fn appends_only_new_tools_after_existing_loading_history() {
        let lookup = json!({"type":"function","name":"lookup","parameters":{"type":"object"}});
        let edit = json!({"type":"custom","name":"edit","format":{"type":"text"}});
        let history = json!([
            {"role":"developer","content":"Synthetic stable context"},
            {"type":"additional_tools","role":"developer","tools":[lookup]},
            {"type":"function_call","name":"lookup","call_id":"call_1","arguments":"{}"},
            {"type":"function_call_output","call_id":"call_1","output":"Synthetic result"},
            {"role":"user","content":"Synthetic next turn"}
        ]);
        let mut request = json!({"input":history,"tools":[lookup,edit],
            "tool_choice":{"type":"custom","name":"edit"},
            "prompt_cache_options":{"mode":"implicit","ttl":"30m"}});
        prepare_siwc_request(&mut request).unwrap();
        let input = request["input"].as_array().unwrap();
        assert_eq!(&input[..5], history.as_array().unwrap());
        assert_eq!(
            input[5],
            json!({"type":"additional_tools","role":"developer","tools":[edit]})
        );
        assert_eq!(request["tools"], json!([]));
        assert_eq!(
            request["tool_choice"],
            json!({"type":"custom","name":"edit"})
        );
        assert_eq!(
            request["prompt_cache_options"],
            json!({"mode":"implicit","ttl":"30m"})
        );
        let prepared = request.clone();
        prepare_siwc_request(&mut request).unwrap();
        assert_eq!(request, prepared);
    }
    #[test]
    fn repeated_current_catalog_does_not_rewrite_or_duplicate_history() {
        let first = json!({"type":"function","name":"first","parameters":{"type":"object"}});
        let second = json!({"type":"custom","name":"second","format":{"type":"text"}});
        let history = json!([
            {"type":"additional_tools","role":"developer","tools":[first]},
            {"role":"user","content":"Synthetic first turn"},
            {"type":"additional_tools","role":"developer","tools":[second]},
            {"role":"user","content":"Synthetic next turn"}
        ]);
        let mut request = json!({"input":history,"tools":[second,first]});
        prepare_siwc_request(&mut request).unwrap();
        assert_eq!(request["input"], history);
    }
    #[test]
    fn current_tool_overrides_latest_history_without_changing_earlier_definitions() {
        let old = json!({"type":"function","name":"lookup","description":"Synthetic old schema","parameters":{"type":"object"}});
        let updated = json!({"type":"function","name":"lookup","description":"Synthetic updated schema","parameters":{"type":"object","required":["value"],"properties":{"value":{"type":"string"}}}});
        for current in [&old, &updated] {
            let history = json!([
                {"type":"additional_tools","role":"developer","tools":[old]},
                {"role":"user","content":"Synthetic first turn"},
                {"type":"additional_tools","role":"developer","tools":[updated]},
                {"role":"user","content":"Synthetic next turn"}
            ]);
            let mut request = json!({"input":history,"tools":[current]});
            prepare_siwc_request(&mut request).unwrap();
            let input = request["input"].as_array().unwrap();
            assert_eq!(&input[..4], history.as_array().unwrap());
            if current == &updated {
                assert_eq!(input.len(), 4);
            } else {
                assert_eq!(
                    input[4],
                    json!({"type":"additional_tools","role":"developer","tools":[current]})
                );
            }
        }
    }
    #[test]
    fn preserves_legacy_calls_missing_a_tool_loading_item() {
        let lookup = json!({"type":"function","name":"lookup","parameters":{"type":"object"}});
        let edit = json!({"type":"custom","name":"edit","format":{"type":"text"}});
        let history = json!([
            {"type":"additional_tools","role":"developer","tools":[lookup]},
            {"type":"custom_tool_call","name":"edit","call_id":"call_edit","input":"Synthetic edit"},
            {"type":"custom_tool_call_output","call_id":"call_edit","output":"Synthetic result"}
        ]);
        let mut request = json!({"input":history,"tools":[lookup,edit]});
        prepare_siwc_request(&mut request).unwrap();
        assert_eq!(
            request["input"][0],
            json!({"type":"additional_tools","role":"developer","tools":[edit]})
        );
        assert_eq!(
            &request["input"].as_array().unwrap()[1..],
            history.as_array().unwrap()
        );
    }
    #[test]
    fn current_schema_property_order_is_not_lost_during_deduplication() {
        let old = json!({"type":"function","name":"lookup","parameters":{
            "type":"object","properties":{"first":{"type":"string"},"second":{"type":"string"}}
        }});
        let current = json!({"type":"function","name":"lookup","parameters":{
            "type":"object","properties":{"second":{"type":"string"},"first":{"type":"string"}}
        }});
        let mut request = json!({"input":[{"type":"additional_tools","role":"developer","tools":[old]}],"tools":[current]});
        prepare_siwc_request(&mut request).unwrap();
        assert_eq!(request["input"].as_array().unwrap().len(), 2);
        let properties = request["input"][1]["tools"][0]["parameters"]["properties"]
            .as_object()
            .unwrap();
        assert_eq!(
            properties.keys().map(String::as_str).collect::<Vec<_>>(),
            ["second", "first"]
        );
    }
    #[test]
    fn rejects_semantic_loss_and_hosted_tools() {
        for field in [
            "temperature",
            "metadata",
            "max_output_tokens",
            "previous_response_id",
            "conversation",
            "store",
        ] {
            let mut request = json!({"input":[]});
            request[field] = json!(true);
            let original = request.clone();
            assert!(prepare_siwc_request(&mut request).is_err(), "{field}");
            assert_eq!(request, original);
        }
        for kind in [
            "tool_search",
            "mcp",
            "computer",
            "code_interpreter",
            "image_generation",
            "file_search",
            "programmatic_tool_calling",
        ] {
            assert!(
                prepare_siwc_request(&mut json!({"input":[], "tools":[{"type":kind}]})).is_err()
            );
        }
    }
}

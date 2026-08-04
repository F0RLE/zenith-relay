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

pub(in crate::gateway) fn normalize_account_request(
    object: &mut Map<String, Value>,
    responses_lite: bool,
) {
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
        filter_responses_lite_tools(object);
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
            for value in values {
                strip_encrypted_reasoning(value, changed);
            }
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

fn filter_responses_lite_tools(object: &mut Map<String, Value>) {
    if let Some(Value::Array(tools)) = object.get_mut("tools") {
        tools.retain_mut(sanitize_responses_lite_tool);
    }
    if let Some(Value::Array(input)) = object.get_mut("input") {
        input.retain_mut(|item| {
            let Some(item) = item.as_object_mut() else {
                return true;
            };
            if item.get("type").and_then(Value::as_str) != Some("additional_tools") {
                return true;
            }
            filter_responses_lite_tools(item);
            item.get("tools")
                .and_then(Value::as_array)
                .is_some_and(|tools| !tools.is_empty())
        });
    }
    if let Some(Value::Object(response)) = object.get_mut("response") {
        filter_responses_lite_tools(response);
    }
    let available_tools = responses_lite_available_tools(object);
    if object
        .get_mut("tool_choice")
        .is_some_and(|choice| !responses_lite_tool_choice_allowed(choice, &available_tools))
    {
        object.remove("tool_choice");
    }
}

fn sanitize_responses_lite_tool(tool: &mut Value) -> bool {
    if !responses_lite_tool_allowed(tool) {
        return false;
    }
    let Some(object) = tool.as_object_mut() else {
        return false;
    };
    if !object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|tool_type| tool_type.eq_ignore_ascii_case("namespace"))
    {
        return true;
    }
    let Some(children) = object.get_mut("tools").and_then(Value::as_array_mut) else {
        return true;
    };
    children.retain_mut(sanitize_responses_lite_namespace_child);
    !children.is_empty()
}

fn sanitize_responses_lite_namespace_child(tool: &mut Value) -> bool {
    if tool.get("type").is_none() {
        return !responses_lite_tool_is_server_executed(tool)
            && tool
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(valid_client_tool_name);
    }
    sanitize_responses_lite_tool(tool)
}

fn responses_lite_available_tools(object: &Map<String, Value>) -> Vec<Value> {
    let mut tools = object
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    if let Some(input) = object.get("input").and_then(Value::as_array) {
        for item in input {
            if item.get("type").and_then(Value::as_str) != Some("additional_tools") {
                continue;
            }
            if let Some(additional) = item.get("tools").and_then(Value::as_array) {
                tools.extend(additional.iter().cloned());
            }
        }
    }
    tools
}

fn responses_lite_tool_allowed(tool: &Value) -> bool {
    let Some(tool_type) = tool
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    if responses_lite_server_tool(tool_type) || responses_lite_tool_is_server_executed(tool) {
        return false;
    }
    if ["function", "custom", "namespace"]
        .iter()
        .any(|allowed| tool_type.eq_ignore_ascii_case(allowed))
    {
        return true;
    }
    if tool_type.eq_ignore_ascii_case("tool_search") {
        // Codex has sent the deferred discovery tool both with and without an
        // explicit execution marker. It is always client-side unless the
        // marker explicitly says server, so do not erase the only route to
        // deferred tools merely because an optional field is absent.
        return true;
    }
    tool.get("name")
        .and_then(Value::as_str)
        .is_some_and(valid_client_tool_name)
}

fn responses_lite_tool_is_server_executed(tool: &Value) -> bool {
    tool.get("execution")
        .and_then(Value::as_str)
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("server"))
}

fn responses_lite_server_tool(tool_type: &str) -> bool {
    let tool_type = tool_type.trim().to_ascii_lowercase();
    [
        "web_search",
        "web_search_preview",
        "image_generation",
        "image_gen",
        "file_search",
        "computer_use_preview",
        "code_interpreter",
        "mcp",
    ]
    .iter()
    .any(|server_tool| tool_type == *server_tool)
        || [
            "web_search_",
            "image_generation_",
            "file_search_",
            "computer_use_",
            "code_interpreter_",
            "mcp_",
        ]
        .iter()
        .any(|prefix| tool_type.starts_with(prefix))
}

fn valid_client_tool_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.trim() == name
        && name.len() <= 256
        && !name.chars().any(char::is_control)
}

fn responses_lite_tool_choice_allowed(choice: &mut Value, available_tools: &[Value]) -> bool {
    if let Some(choice) = choice.as_str() {
        let choice = choice.trim();
        return if choice.eq_ignore_ascii_case("auto") || choice.eq_ignore_ascii_case("none") {
            true
        } else if choice.eq_ignore_ascii_case("required") {
            !available_tools.is_empty()
        } else {
            false
        };
    }
    let Some(choice) = choice.as_object_mut() else {
        return false;
    };
    let Some(choice_type) = choice.get("type").and_then(Value::as_str).map(str::trim) else {
        return false;
    };
    if !choice_type.eq_ignore_ascii_case("allowed_tools") {
        return responses_lite_tool_choice_matches_available(choice, available_tools);
    }
    let mut any_allowed = false;
    for name in ["tools", "allowed_tools"] {
        if let Some(tools) = choice.get_mut(name).and_then(Value::as_array_mut) {
            tools.retain(|tool| {
                tool.as_object().is_some_and(|tool| {
                    responses_lite_tool_choice_matches_available(tool, available_tools)
                })
            });
            any_allowed |= !tools.is_empty();
        }
    }
    any_allowed
}

fn responses_lite_tool_choice_matches_available(
    choice: &serde_json::Map<String, Value>,
    available_tools: &[Value],
) -> bool {
    let Some(choice_type) = choice.get("type").and_then(Value::as_str).map(str::trim) else {
        return false;
    };
    if responses_lite_server_tool(choice_type)
        || choice
            .get("execution")
            .and_then(Value::as_str)
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("server"))
    {
        return false;
    }
    available_tools
        .iter()
        .any(|tool| responses_lite_tool_choice_matches_definition(choice, tool, None))
}

fn responses_lite_tool_choice_matches_definition(
    choice: &serde_json::Map<String, Value>,
    definition: &Value,
    namespace: Option<&str>,
) -> bool {
    let Some(definition) = definition.as_object() else {
        return false;
    };
    let Some(definition_type) = definition
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
    else {
        return false;
    };
    if definition_type.eq_ignore_ascii_case("namespace") {
        let Some(definition_namespace) = definition
            .get("name")
            .or_else(|| definition.get("namespace"))
            .and_then(Value::as_str)
            .filter(|value| valid_client_tool_name(value))
        else {
            return false;
        };
        if choice
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|choice_type| choice_type.eq_ignore_ascii_case("namespace"))
            && choice
                .get("name")
                .or_else(|| choice.get("namespace"))
                .and_then(Value::as_str)
                == Some(definition_namespace)
        {
            return true;
        }
        return definition
            .get("tools")
            .and_then(Value::as_array)
            .is_some_and(|children| {
                children.iter().any(|child| {
                    responses_lite_tool_choice_matches_namespace_child(
                        choice,
                        child,
                        definition_namespace,
                    )
                })
            });
    }
    responses_lite_tool_choice_matches_leaf(choice, definition, namespace)
}

fn responses_lite_tool_choice_matches_namespace_child(
    choice: &serde_json::Map<String, Value>,
    definition: &Value,
    namespace: &str,
) -> bool {
    let Some(object) = definition.as_object() else {
        return false;
    };
    if object.get("type").is_none() {
        return responses_lite_tool_choice_matches_leaf(choice, object, Some(namespace));
    }
    responses_lite_tool_choice_matches_definition(choice, definition, Some(namespace))
}

fn responses_lite_tool_choice_matches_leaf(
    choice: &serde_json::Map<String, Value>,
    definition: &serde_json::Map<String, Value>,
    namespace: Option<&str>,
) -> bool {
    let Some(choice_type) = choice.get("type").and_then(Value::as_str).map(str::trim) else {
        return false;
    };
    let definition_type = definition
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("function");
    if !choice_type.eq_ignore_ascii_case(definition_type) {
        return false;
    }
    if namespace.is_some() && choice.get("namespace").and_then(Value::as_str) != namespace {
        return false;
    }
    let definition_name = definition
        .get("name")
        .or_else(|| {
            definition
                .get("function")
                .and_then(|function| function.get("name"))
        })
        .and_then(Value::as_str);
    let choice_name = choice
        .get("name")
        .or_else(|| {
            choice
                .get("function")
                .and_then(|function| function.get("name"))
        })
        .and_then(Value::as_str);
    match (definition_name, choice_name) {
        (None, None) => choice_type.eq_ignore_ascii_case("tool_search"),
        (Some(definition_name), Some(choice_name)) => definition_name == choice_name,
        _ => false,
    }
}

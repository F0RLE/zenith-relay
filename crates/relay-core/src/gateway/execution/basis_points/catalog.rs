use serde_json::{Map, Value};

#[derive(Clone, Debug)]
pub(super) struct ClientTool {
    pub(super) name: String,
    pub(super) namespace: Option<String>,
    pub(super) kind: String,
    pub(super) spec: Map<String, Value>,
}

impl ClientTool {
    pub(super) fn key(&self) -> String {
        self.namespace
            .as_ref()
            .map(|namespace| format!("{namespace}.{}", self.name))
            .unwrap_or_else(|| self.name.clone())
    }

    pub(super) fn call_name(&self) -> String {
        self.key()
    }
}

pub(super) fn collect_tools(
    tool_value: Option<&Value>,
    namespace: Option<&str>,
    collected_tools: &mut Vec<ClientTool>,
) {
    let Some(Value::Array(tool_values)) = tool_value else {
        return;
    };
    for tool_value in tool_values {
        let Some(tool_object) = tool_value.as_object() else {
            continue;
        };
        let tool_kind = tool_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("function")
            .trim()
            .to_ascii_lowercase();
        let Some(tool_name) = tool_object.get("name").and_then(Value::as_str) else {
            if tool_kind == "namespace" {
                continue;
            }
            continue;
        };
        let tool_name = tool_name.trim();
        if tool_name.is_empty() {
            continue;
        }
        if tool_kind == "namespace" {
            collect_tools(tool_object.get("tools"), Some(tool_name), collected_tools);
        } else if matches!(tool_kind.as_str(), "function" | "custom") {
            let tool = ClientTool {
                name: tool_name.to_string(),
                namespace: namespace.map(str::to_string),
                kind: tool_kind,
                spec: tool_object.clone(),
            };
            // A later additional_tools item replaces the earlier definition
            // for this qualified name, including its kind and schema.
            collected_tools.retain(|existing_tool| existing_tool.key() != tool.key());
            collected_tools.push(tool);
        }
    }
}

pub(super) fn client_tools(request_body: &Value) -> Vec<ClientTool> {
    let mut client_tools = Vec::new();
    collect_tools(request_body.get("tools"), None, &mut client_tools);
    if let Some(input_items) = request_body.get("input").and_then(Value::as_array) {
        for input_item in input_items {
            if input_item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                collect_tools(input_item.get("tools"), None, &mut client_tools);
            }
        }
    }
    client_tools
}

pub(super) fn selected_tools(request_body: &Value, tools: &[ClientTool]) -> Vec<ClientTool> {
    let Some(choice_value) = request_body.get("tool_choice") else {
        return tools.to_vec();
    };
    if choice_value.as_str() == Some("none") {
        return Vec::new();
    }
    let Some(choice_object) = choice_value.as_object() else {
        return tools.to_vec();
    };
    if choice_object.get("type").and_then(Value::as_str) == Some("allowed_tools") {
        let Some(allowed_tools) = choice_object.get("tools").and_then(Value::as_array) else {
            return Vec::new();
        };
        return tools
            .iter()
            .filter(|tool| {
                allowed_tools
                    .iter()
                    .any(|allowed_tool| choice_matches_tool(allowed_tool, tool))
            })
            .cloned()
            .collect();
    }
    if choice_object.get("type").and_then(Value::as_str) == Some("auto") {
        return tools.to_vec();
    }
    tools
        .iter()
        .filter(|tool| choice_matches_tool(choice_value, tool))
        .cloned()
        .collect()
}

pub(super) fn choice_matches_tool(choice_value: &Value, tool: &ClientTool) -> bool {
    let Some(tool_name) = choice_value.get("name").and_then(Value::as_str) else {
        return false;
    };
    let name_matches = match choice_value.get("namespace").and_then(Value::as_str) {
        Some(namespace) => tool.namespace.as_deref() == Some(namespace) && tool.name == tool_name,
        None => tool_name == tool.name || tool_name == tool.key(),
    };
    name_matches
        && choice_value
            .get("type")
            .and_then(Value::as_str)
            .is_none_or(|kind| kind == tool.kind)
}

pub(super) fn requires_tool_call(tool_choice: Option<&Value>) -> bool {
    tool_choice.is_some_and(|tool_choice| {
        tool_choice.as_str() == Some("required")
            || tool_choice.as_object().is_some_and(|tool_choice| {
                matches!(
                    tool_choice.get("type").and_then(Value::as_str),
                    Some("function" | "custom")
                ) || (tool_choice.get("type").and_then(Value::as_str) == Some("allowed_tools")
                    && tool_choice.get("mode").and_then(Value::as_str) == Some("required"))
            })
    })
}

pub(super) fn tool_spec<'a>(
    tools: &'a [ClientTool],
    requested_tool_name: &str,
) -> Option<&'a ClientTool> {
    // A qualified namespace is authoritative. A bare name is only safe when
    // it identifies one tool; otherwise it could dispatch to the wrong client.
    if let Some(tool) = tools.iter().find(|tool| tool.key() == requested_tool_name) {
        return Some(tool);
    }
    let mut matches = tools.iter().filter(|tool| tool.name == requested_tool_name);
    let tool = matches.next()?;
    matches.next().is_none().then_some(tool)
}

pub(super) fn client_tool_call_name(tool_call_object: &Map<String, Value>) -> String {
    let client_tool_name = tool_call_object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let namespace = tool_call_object
        .get("namespace")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if namespace.is_empty()
        || client_tool_name.is_empty()
        || client_tool_name.starts_with(&format!("{namespace}."))
    {
        client_tool_name.to_string()
    } else {
        format!("{namespace}.{client_tool_name}")
    }
}

/// A later turn, including compaction, may no longer carry the tool catalog.
/// The historical call still has its name and payload, so it can be restored
/// to the native transport without looking the tool up again.
pub(super) fn history_tool(tool_call_object: &Map<String, Value>) -> Option<ClientTool> {
    // Reuse the qualified client name. Storing the namespace again would turn
    // an already qualified historical call into namespace.namespace.name.
    let qualified_tool_name = client_tool_call_name(tool_call_object);
    if qualified_tool_name.is_empty() {
        return None;
    }
    let kind = if tool_call_object.get("type").and_then(Value::as_str) == Some("custom_tool_call") {
        "custom"
    } else {
        "function"
    };
    Some(ClientTool {
        name: qualified_tool_name,
        namespace: None,
        kind: kind.to_string(),
        spec: Map::new(),
    })
}

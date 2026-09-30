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
    value: Option<&Value>,
    namespace: Option<&str>,
    output: &mut Vec<ClientTool>,
) {
    let Some(Value::Array(tools)) = value else {
        return;
    };
    for value in tools {
        let Some(object) = value.as_object() else {
            continue;
        };
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("function")
            .trim()
            .to_ascii_lowercase();
        let Some(name) = object.get("name").and_then(Value::as_str) else {
            if kind == "namespace" {
                continue;
            }
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if kind == "namespace" {
            collect_tools(object.get("tools"), Some(name), output);
        } else if matches!(kind.as_str(), "function" | "custom") {
            let tool = ClientTool {
                name: name.to_string(),
                namespace: namespace.map(str::to_string),
                kind,
                spec: object.clone(),
            };
            // A later additional_tools item replaces the earlier definition
            // for this qualified name, including its kind and schema.
            output.retain(|previous| previous.key() != tool.key());
            output.push(tool);
        }
    }
}

pub(super) fn client_tools(request: &Value) -> Vec<ClientTool> {
    let mut result = Vec::new();
    collect_tools(request.get("tools"), None, &mut result);
    if let Some(items) = request.get("input").and_then(Value::as_array) {
        for item in items {
            if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                collect_tools(item.get("tools"), None, &mut result);
            }
        }
    }
    result
}

pub(super) fn selected_tools(request: &Value, tools: &[ClientTool]) -> Vec<ClientTool> {
    let Some(choice) = request.get("tool_choice") else {
        return tools.to_vec();
    };
    if choice.as_str() == Some("none") {
        return Vec::new();
    }
    let Some(object) = choice.as_object() else {
        return tools.to_vec();
    };
    if object.get("type").and_then(Value::as_str) == Some("allowed_tools") {
        let Some(allowed) = object.get("tools").and_then(Value::as_array) else {
            return Vec::new();
        };
        return tools
            .iter()
            .filter(|tool| {
                allowed
                    .iter()
                    .any(|allowed| choice_matches_tool(allowed, tool))
            })
            .cloned()
            .collect();
    }
    if object.get("type").and_then(Value::as_str) == Some("auto") {
        return tools.to_vec();
    }
    tools
        .iter()
        .filter(|tool| choice_matches_tool(choice, tool))
        .cloned()
        .collect()
}

pub(super) fn choice_matches_tool(choice: &Value, tool: &ClientTool) -> bool {
    let Some(name) = choice.get("name").and_then(Value::as_str) else {
        return false;
    };
    let name_matches = match choice.get("namespace").and_then(Value::as_str) {
        Some(namespace) => tool.namespace.as_deref() == Some(namespace) && tool.name == name,
        None => name == tool.name || name == tool.key(),
    };
    name_matches
        && choice
            .get("type")
            .and_then(Value::as_str)
            .is_none_or(|kind| kind == tool.kind)
}

pub(super) fn requires_tool_call(choice: Option<&Value>) -> bool {
    choice.is_some_and(|choice| {
        choice.as_str() == Some("required")
            || choice.as_object().is_some_and(|choice| {
                matches!(
                    choice.get("type").and_then(Value::as_str),
                    Some("function" | "custom")
                ) || (choice.get("type").and_then(Value::as_str) == Some("allowed_tools")
                    && choice.get("mode").and_then(Value::as_str) == Some("required"))
            })
    })
}

pub(super) fn tool_spec<'a>(tools: &'a [ClientTool], name: &str) -> Option<&'a ClientTool> {
    // A qualified namespace is authoritative. A bare name is only safe when
    // it identifies one tool; otherwise it could dispatch to the wrong client.
    if let Some(tool) = tools.iter().find(|tool| tool.key() == name) {
        return Some(tool);
    }
    let mut matches = tools.iter().filter(|tool| tool.name == name);
    let tool = matches.next()?;
    matches.next().is_none().then_some(tool)
}

pub(super) fn client_tool_call_name(object: &Map<String, Value>) -> String {
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let namespace = object
        .get("namespace")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    if namespace.is_empty() || name.is_empty() || name.starts_with(&format!("{namespace}.")) {
        name.to_string()
    } else {
        format!("{namespace}.{name}")
    }
}

/// A later turn, including compaction, may no longer carry the tool catalog.
/// The historical call still has its name and payload, so it can be restored
/// to the native transport without looking the tool up again.
pub(super) fn history_tool(object: &Map<String, Value>) -> Option<ClientTool> {
    // Reuse the qualified client name. Storing the namespace again would turn
    // an already qualified historical call into namespace.namespace.name.
    let name = client_tool_call_name(object);
    if name.is_empty() {
        return None;
    }
    let kind = if object.get("type").and_then(Value::as_str) == Some("custom_tool_call") {
        "custom"
    } else {
        "function"
    };
    Some(ClientTool {
        name,
        namespace: None,
        kind: kind.to_string(),
        spec: Map::new(),
    })
}

use crate::protocol::AdapterError;
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

mod prepared;
pub(super) use prepared::{cache_memory_diagnostics, release_expired_cache_entries};

/// Parsed declarations for one immutable client request, never a permission cache.
/// Selection and argument validation still use that request at every boundary.
#[derive(Clone, Default)]
pub(in crate::gateway) struct RequestCatalog {
    tools: Option<Arc<[ClientTool]>>,
}

impl RequestCatalog {
    pub(super) fn tools(&mut self, request: &Value) -> Result<&[ClientTool], AdapterError> {
        if self.tools.is_none() {
            self.tools = Some(client_tools(request)?.into());
        }
        Ok(self.tools.as_deref().expect("catalog was prepared"))
    }
}

/// Owns the exact body from which the catalog was prepared. Only immutable
/// access is exposed, so discovery and final validation cannot see stale tools.
pub(in crate::gateway) struct PreparedClientRequest {
    body: Value,
    catalog: RequestCatalog,
    agent_messages: Option<crate::protocol::agent_messages::AgentMessageContext>,
}

impl PreparedClientRequest {
    pub(in crate::gateway::execution) fn prepare(
        body: Value,
        schema_validation_ms: &mut Option<u64>,
        diagnostics_enabled: bool,
    ) -> Result<(Self, Value), AdapterError> {
        let mut catalog = RequestCatalog::default();
        let upstream = super::prepare::prepare_request_with_catalog(
            &body,
            schema_validation_ms,
            diagnostics_enabled,
            &mut catalog,
        )?;
        Ok((
            Self {
                body,
                catalog,
                agent_messages: None,
            },
            upstream,
        ))
    }

    pub(in crate::gateway) fn with_agent_messages(
        mut self,
        context: crate::protocol::agent_messages::AgentMessageContext,
    ) -> Self {
        self.agent_messages = Some(context);
        self
    }

    pub(in crate::gateway) fn translate_value(
        &self,
        response: Value,
    ) -> Result<Value, AdapterError> {
        let mut catalog = self.catalog.clone();
        super::response::translate_response_with_catalog(
            response,
            &self.body,
            &mut catalog,
            self.agent_messages.as_ref(),
        )
    }

    /// Share immutable declarations with dispatch without exposing the cached
    /// catalog for replacement independently of its owning request body.
    pub(in crate::gateway::execution) fn catalog_snapshot(&self) -> RequestCatalog {
        self.catalog.clone()
    }

    pub(in crate::gateway::execution) fn translate_bytes(
        &self,
        response: &[u8],
    ) -> Result<Vec<u8>, AdapterError> {
        super::response::translate_response_bytes_with_catalog(
            response,
            &self.body,
            &mut self.catalog.clone(),
            self.agent_messages.as_ref(),
        )
    }
}

impl std::ops::Deref for PreparedClientRequest {
    type Target = Value;

    fn deref(&self) -> &Value {
        &self.body
    }
}

#[cfg(test)]
impl From<Value> for PreparedClientRequest {
    fn from(body: Value) -> Self {
        Self {
            body,
            catalog: RequestCatalog::default(),
            agent_messages: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ClientTool {
    pub(super) name: String,
    pub(super) namespace: Option<String>,
    pub(super) kind: String,
    pub(super) spec: Arc<Map<String, Value>>,
    pub(super) serialized_spec: Arc<str>,
    schema_size: Arc<OnceLock<u64>>,
    schema_validated: Arc<OnceLock<()>>,
    presentation: Arc<OnceLock<prepared::Presentation>>,
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

    pub(super) fn validate_parameter_schema(&self) -> Result<(), AdapterError> {
        if self.schema_validated.get().is_some() {
            return Ok(());
        }
        if let Some(schema) = super::schema::parameter_schema(self) {
            super::agent_messages::validate_schema(schema)?;
            super::schema::validator(schema)
                .map_err(|error| super::schema::validation_error(&error, true))?;
        }
        // The content-addressed declaration is immutable. Remember successful
        // compilation, not a compiled validator or any per-request permission.
        let _ = self.schema_validated.set(());
        Ok(())
    }
}

struct ToolDeclaration<'a> {
    name: &'a str,
    namespace: Option<String>,
    kind: String,
    spec: &'a Map<String, Value>,
}

impl ToolDeclaration<'_> {
    fn prepare(self) -> ClientTool {
        prepared::tool(self.name, self.namespace.as_deref(), &self.kind, self.spec)
    }
}

fn collect_tools<'a>(
    tool_value: Option<&'a Value>,
    namespace: Option<&str>,
    collected_tools: &mut Vec<ToolDeclaration<'a>>,
    positions: &mut HashMap<String, usize>,
) -> Result<(), AdapterError> {
    let Some(tool_value) = tool_value else {
        return Ok(());
    };
    let tool_values = tool_value
        .as_array()
        .ok_or_else(|| AdapterError::invalid_request().with_parameter("tools"))?;
    for tool_value in tool_values {
        let tool_object = tool_value
            .as_object()
            .ok_or_else(|| AdapterError::invalid_request().with_parameter("tools"))?;
        let tool_kind = tool_object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("function")
            .trim()
            .to_ascii_lowercase();
        if !matches!(tool_kind.as_str(), "namespace" | "function" | "custom") {
            // Built-in tools are not part of the client callable catalog.
            // A named unsupported declaration cannot revive a historical tool.
            if tool_object.contains_key("name") {
                return Err(AdapterError::invalid_request().with_parameter("tools.type"));
            }
            continue;
        }
        let tool_name = tool_object
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| AdapterError::invalid_request().with_parameter("tools.name"))?;
        let tool_name = tool_name.trim();
        if tool_name.is_empty() {
            return Err(AdapterError::invalid_request().with_parameter("tools.name"));
        }
        if tool_kind == "namespace" {
            let qualified_namespace = namespace
                .map(|parent| format!("{parent}.{tool_name}"))
                .unwrap_or_else(|| tool_name.to_string());
            if !tool_object.contains_key("tools") {
                return Err(AdapterError::invalid_request().with_parameter("tools"));
            }
            collect_tools(
                tool_object.get("tools"),
                Some(&qualified_namespace),
                collected_tools,
                positions,
            )?;
        } else if matches!(tool_kind.as_str(), "function" | "custom") {
            let tool = ToolDeclaration {
                name: tool_name,
                namespace: namespace.map(str::to_owned),
                kind: tool_kind,
                spec: tool_object,
            };
            // Replace the definition without moving its catalog position.
            // Borrow until overrides are resolved: superseded history must not
            // be serialized, cloned or retained in the presentation cache.
            let key = namespace
                .map(|namespace| format!("{namespace}.{tool_name}"))
                .unwrap_or_else(|| tool_name.to_owned());
            if let Some(&position) = positions.get(&key) {
                collected_tools[position] = tool;
            } else {
                positions.insert(key, collected_tools.len());
                collected_tools.push(tool);
            }
        }
    }
    Ok(())
}

pub(super) fn client_tools(request_body: &Value) -> Result<Vec<ClientTool>, AdapterError> {
    let mut client_tools = Vec::new();
    let mut positions = HashMap::new();
    if let Some(input_items) = request_body.get("input").and_then(Value::as_array) {
        for input_item in input_items {
            if matches!(
                input_item.get("type").and_then(Value::as_str),
                Some("additional_tools" | "tool_search_output")
            ) {
                collect_tools(
                    input_item.get("tools"),
                    None,
                    &mut client_tools,
                    &mut positions,
                )?;
            }
        }
    }
    // Current declarations are authoritative; history fills omitted tools only.
    collect_tools(
        request_body.get("tools"),
        None,
        &mut client_tools,
        &mut positions,
    )?;
    Ok(client_tools
        .into_iter()
        .map(ToolDeclaration::prepare)
        .collect())
}

pub(super) fn declared_tools(value: Option<&Value>) -> Result<Vec<ClientTool>, AdapterError> {
    let mut tools = Vec::new();
    collect_tools(value, None, &mut tools, &mut HashMap::new())?;
    Ok(tools.into_iter().map(ToolDeclaration::prepare).collect())
}

pub(super) fn has_historical_declarations(request: &Value) -> bool {
    request["input"].as_array().is_some_and(|items| {
        items.iter().any(|item| {
            matches!(
                item["type"].as_str(),
                Some("additional_tools" | "tool_search_output")
            )
        })
    })
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
        spec: Arc::new(Map::new()),
        serialized_spec: Arc::from("{}"),
        schema_size: Arc::new(OnceLock::new()),
        schema_validated: Arc::new(OnceLock::new()),
        presentation: Arc::new(OnceLock::new()),
    })
}

#[cfg(test)]
mod tests;

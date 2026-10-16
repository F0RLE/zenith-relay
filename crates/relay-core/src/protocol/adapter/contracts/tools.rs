use super::{AdapterError, AdapterResult};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
pub(in crate::protocol::adapter) fn bridged_namespace_tool_name(
    namespace: &str,
    tool_name: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update((namespace.len() as u64).to_le_bytes());
    hasher.update(namespace.as_bytes());
    hasher.update((tool_name.len() as u64).to_le_bytes());
    hasher.update(tool_name.as_bytes());
    let digest = hasher.finalize();
    format!("relay_ns_{}", hex::encode(&digest[..12]))
}

/// Keep namespace context in a flat upstream tool's description.
pub(in crate::protocol::adapter) fn bridged_tool_description(
    tool: &Map<String, Value>,
    namespace: Option<&str>,
    namespace_description: Option<&str>,
    tool_name: &str,
) -> Option<String> {
    let tool_description = tool
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|description| !description.is_empty());
    let Some(namespace) = namespace else {
        return tool_description.map(str::to_owned);
    };
    let mut description = format!("Namespace `{namespace}` tool `{tool_name}`.");
    for text in [namespace_description, tool_description]
        .into_iter()
        .flatten()
    {
        description.push(' ');
        description.push_str(text);
    }
    Some(description)
}

/// Collects the complete client-side tool catalog for one Responses request.
/// Historical discoveries fill omitted tools; the current root declarations
/// override them without duplicating a tool or changing its catalog position.
pub(in crate::protocol::adapter) fn request_tool_catalog(
    request_object: &Map<String, Value>,
) -> AdapterResult<Option<Vec<Value>>> {
    let mut declared = false;
    let mut tools = Vec::new();
    if let Some(input_items) = request_object.get("input").and_then(Value::as_array) {
        for additional_tools_item in input_items {
            if !matches!(
                additional_tools_item.get("type").and_then(Value::as_str),
                Some("additional_tools" | "tool_search_output")
            ) {
                continue;
            }
            declared = true;
            merge_tool_definitions(
                &mut tools,
                additional_tools_item
                    .get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(AdapterError::invalid_request)?,
            )?;
        }
    }
    if let Some(root_tools) = request_object.get("tools").filter(|tools| !tools.is_null()) {
        declared = true;
        merge_tool_definitions(
            &mut tools,
            root_tools
                .as_array()
                .ok_or_else(AdapterError::invalid_request)?,
        )?;
    }
    Ok(declared.then_some(tools))
}

fn merge_tool_definitions(catalog: &mut Vec<Value>, definitions: &[Value]) -> AdapterResult<()> {
    let mut names = BTreeSet::new();
    for definition in definitions {
        super::validation::validate_bridge_tool(definition)?;
        let fields = definition
            .as_object()
            .ok_or_else(AdapterError::unsupported_tool)?;
        let kind = fields
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("function");
        let name = fields.get("name").and_then(Value::as_str).map(str::trim);
        let is_namespace = kind == "namespace";
        if name.is_some_and(|name| !names.insert((is_namespace, name))) {
            return Err(AdapterError::invalid_request());
        }
        let existing = name.and_then(|name| {
            catalog.iter_mut().find(|tool| {
                tool.get("name").and_then(Value::as_str).map(str::trim) == Some(name)
                    && (tool.get("type").and_then(Value::as_str) == Some("namespace"))
                        == is_namespace
            })
        });
        if let Some(existing) = existing {
            let mut replacement = definition.clone();
            if is_namespace {
                let mut children = existing
                    .get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(AdapterError::unsupported_tool)?
                    .clone();
                merge_tool_definitions(
                    &mut children,
                    fields
                        .get("tools")
                        .and_then(Value::as_array)
                        .ok_or_else(AdapterError::unsupported_tool)?,
                )?;
                replacement["tools"] = Value::Array(children);
            }
            *existing = replacement;
        } else {
            let mut definition = definition.clone();
            if is_namespace {
                let mut children = Vec::new();
                merge_tool_definitions(
                    &mut children,
                    fields
                        .get("tools")
                        .and_then(Value::as_array)
                        .ok_or_else(AdapterError::unsupported_tool)?,
                )?;
                definition["tools"] = Value::Array(children);
            }
            catalog.push(definition);
        }
    }
    Ok(())
}

/// The original Responses contract expected by the client for one tool name.
///
/// Anthropic always represents a tool invocation as an object, so a direct
/// custom tool uses the internal `input` string field until it is translated
/// back at the client boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::protocol::adapter) enum ResponsesToolKind {
    Function,
    Custom,
}

/// One client-visible tool represented by an upstream Messages tool name.
///
/// Namespace functions have only a local name inside the Responses contract,
/// while Messages requires one flat, globally unique tool name. The bridge
/// therefore records both identities and never asks the client to execute an
/// opaque generated upstream name.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(in crate::protocol::adapter) struct ClientToolTarget {
    pub(in crate::protocol::adapter) kind: ResponsesToolKind,
    pub(in crate::protocol::adapter) name: String,
    pub(in crate::protocol::adapter) namespace: Option<String>,
}

impl ClientToolTarget {
    pub(in crate::protocol::adapter) fn from_call(
        call: &Map<String, Value>,
    ) -> AdapterResult<Self> {
        let namespace = call
            .get("namespace")
            .filter(|value| !value.is_null())
            .map(|value| {
                value
                    .as_str()
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .ok_or_else(AdapterError::invalid_request)
            })
            .transpose()?;
        Ok(Self {
            kind: ResponsesToolKind::from_call_item(call)?,
            name: call
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .ok_or_else(AdapterError::invalid_request)?
                .to_owned(),
            namespace: namespace.map(str::to_owned),
        })
    }

    pub(in crate::protocol::adapter) fn from_definition(
        tool: &Map<String, Value>,
        namespace: Option<&str>,
    ) -> AdapterResult<Self> {
        let kind = ResponsesToolKind::from_definition(tool)?;
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(AdapterError::unsupported_tool)?;
        Ok(Self {
            kind,
            name: name.to_owned(),
            namespace: namespace.map(str::to_owned),
        })
    }

    pub(in crate::protocol::adapter) fn upstream_name(&self) -> String {
        self.namespace
            .as_deref()
            .map(|namespace| bridged_namespace_tool_name(namespace, &self.name))
            .unwrap_or_else(|| self.name.clone())
    }
}

impl ResponsesToolKind {
    pub(in crate::protocol::adapter) fn from_definition(
        tool: &Map<String, Value>,
    ) -> AdapterResult<Self> {
        match tool.get("type").and_then(Value::as_str) {
            Some("function") => Ok(Self::Function),
            Some("custom") => Ok(Self::Custom),
            // Responses namespace children have historically omitted `type`
            // for ordinary client functions. A named, untyped definition is
            // still representable as a JSON-schema function for Messages.
            None if tool
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|tool_name| !tool_name.trim().is_empty()) =>
            {
                Ok(Self::Function)
            }
            _ => Err(AdapterError::unsupported_tool()),
        }
    }

    pub(in crate::protocol::adapter) fn from_call_item(
        call_item: &Map<String, Value>,
    ) -> AdapterResult<Self> {
        match call_item.get("type").and_then(Value::as_str) {
            Some("function_call") => Ok(Self::Function),
            Some("custom_tool_call") => Ok(Self::Custom),
            _ => Err(AdapterError::invalid_request()),
        }
    }

    pub(in crate::protocol::adapter) fn from_output_item(
        output_item: &Map<String, Value>,
    ) -> AdapterResult<Self> {
        match output_item.get("type").and_then(Value::as_str) {
            Some("function_call_output") => Ok(Self::Function),
            Some("custom_tool_call_output") => Ok(Self::Custom),
            _ => Err(AdapterError::invalid_request()),
        }
    }
}

#[derive(Clone, Debug)]
pub(in crate::protocol::adapter) enum ToolChoice {
    Auto,
    None,
    Required,
    Function(String),
}

/// Resolves Responses choices before encoding a provider's contract. An empty
/// allowed set disables callable tools; it must never reopen the full catalog.
pub(in crate::protocol::adapter) fn resolve_responses_tool_choice(
    choice: &Value,
    available: &BTreeMap<String, ClientToolTarget>,
) -> AdapterResult<(Option<ToolChoice>, Option<BTreeSet<String>>)> {
    if choice.is_null() {
        return Ok((None, None));
    }
    if let Some(mode) = choice.as_str() {
        let choice = match mode {
            "auto" => ToolChoice::Auto,
            "none" => ToolChoice::None,
            "required" if !available.is_empty() => ToolChoice::Required,
            _ => return Err(AdapterError::unsupported_tool()),
        };
        let allowed = matches!(choice, ToolChoice::None).then(BTreeSet::new);
        return Ok((Some(choice), allowed));
    }
    let fields = choice
        .as_object()
        .ok_or_else(AdapterError::unsupported_tool)?;
    if fields.get("type").and_then(Value::as_str) == Some("allowed_tools") {
        super::validation::validate_bridge_fields(choice, &["type", "mode", "tools"])?;
        let mode = match fields
            .get("mode")
            .map(Value::as_str)
            .unwrap_or(Some("auto"))
        {
            Some("auto") => ToolChoice::Auto,
            Some("required") => ToolChoice::Required,
            _ => return Err(AdapterError::unsupported_tool()),
        };
        let mut allowed = BTreeSet::new();
        for tool in fields
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(AdapterError::unsupported_tool)?
        {
            allowed.extend(selected_tool_names(tool, available)?);
        }
        if allowed.is_empty() && matches!(mode, ToolChoice::Required) {
            return Err(AdapterError::unsupported_tool());
        }
        return Ok(((!allowed.is_empty()).then_some(mode), Some(allowed)));
    }
    let allowed = selected_tool_names(choice, available)?;
    if allowed.is_empty() {
        return Err(AdapterError::unsupported_tool());
    }
    if fields.get("type").and_then(Value::as_str) == Some("namespace") {
        Ok((Some(ToolChoice::Required), Some(allowed)))
    } else {
        let name = allowed.first().cloned().expect("selected tool");
        Ok((Some(ToolChoice::Function(name)), Some(allowed)))
    }
}

fn selected_tool_names(
    choice: &Value,
    available: &BTreeMap<String, ClientToolTarget>,
) -> AdapterResult<BTreeSet<String>> {
    let fields = choice
        .as_object()
        .ok_or_else(AdapterError::unsupported_tool)?;
    match fields.get("type").and_then(Value::as_str) {
        Some("namespace") => {
            super::validation::validate_bridge_fields(choice, &["type", "name", "namespace"])?;
            let namespace = fields
                .get("name")
                .or_else(|| fields.get("namespace"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|namespace| !namespace.is_empty())
                .ok_or_else(AdapterError::unsupported_tool)?;
            let names = available
                .iter()
                .filter_map(|(name, target)| {
                    (target.namespace.as_deref() == Some(namespace)).then_some(name.clone())
                })
                .collect::<BTreeSet<_>>();
            if names.is_empty() {
                return Err(AdapterError::unsupported_tool());
            }
            Ok(names)
        }
        Some("function" | "custom") => {
            super::validation::validate_bridge_fields(choice, &["type", "name", "namespace"])?;
            let namespace = fields
                .get("namespace")
                .filter(|namespace| !namespace.is_null())
                .map(|namespace| {
                    namespace
                        .as_str()
                        .map(str::trim)
                        .filter(|namespace| !namespace.is_empty())
                        .ok_or_else(AdapterError::unsupported_tool)
                })
                .transpose()?;
            let target = ClientToolTarget::from_definition(fields, namespace)?;
            let name = target.upstream_name();
            if available.get(&name) != Some(&target) {
                return Err(AdapterError::unsupported_tool());
            }
            Ok(BTreeSet::from([name]))
        }
        // Hosted tools are absent from a bridged callable catalog.
        Some(_) => Ok(BTreeSet::new()),
        None => Err(AdapterError::unsupported_tool()),
    }
}

use super::{AdapterError, AdapterResult};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
pub(in crate::protocol::adapter) fn bridged_namespace_tool_name(
    namespace: &str,
    name: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update((namespace.len() as u64).to_le_bytes());
    hasher.update(namespace.as_bytes());
    hasher.update((name.len() as u64).to_le_bytes());
    hasher.update(name.as_bytes());
    let digest = hasher.finalize();
    format!("relay_ns_{}", hex::encode(&digest[..12]))
}

/// Collects the complete client-side tool catalog for one Responses request.
/// Codex can place tools loaded during a turn in `input.additional_tools`;
/// bridges need to combine those with the root catalog before they translate
/// their distinct upstream contracts.
pub(in crate::protocol::adapter) fn request_tool_catalog(
    object: &Map<String, Value>,
) -> AdapterResult<Option<Vec<Value>>> {
    let mut declared = false;
    let mut tools = Vec::new();
    if let Some(root) = object.get("tools") {
        declared = true;
        tools.extend(
            root.as_array()
                .ok_or_else(AdapterError::invalid_request)?
                .iter()
                .cloned(),
        );
    }
    if let Some(input) = object.get("input").and_then(Value::as_array) {
        for item in input {
            if item.get("type").and_then(Value::as_str) != Some("additional_tools") {
                continue;
            }
            declared = true;
            tools.extend(
                item.get("tools")
                    .and_then(Value::as_array)
                    .ok_or_else(AdapterError::invalid_request)?
                    .iter()
                    .cloned(),
            );
        }
    }
    Ok(declared.then_some(tools))
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
                .is_some_and(|name| !name.trim().is_empty()) =>
            {
                Ok(Self::Function)
            }
            _ => Err(AdapterError::unsupported_tool()),
        }
    }

    pub(in crate::protocol::adapter) fn from_call_item(
        item: &Map<String, Value>,
    ) -> AdapterResult<Self> {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => Ok(Self::Function),
            Some("custom_tool_call") => Ok(Self::Custom),
            _ => Err(AdapterError::invalid_request()),
        }
    }

    pub(in crate::protocol::adapter) fn from_output_item(
        item: &Map<String, Value>,
    ) -> AdapterResult<Self> {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call_output") => Ok(Self::Function),
            Some("custom_tool_call_output") => Ok(Self::Custom),
            _ => Err(AdapterError::invalid_request()),
        }
    }

    pub(in crate::protocol::adapter) const fn response_item_type(self) -> &'static str {
        match self {
            Self::Function => "function_call",
            Self::Custom => "custom_tool_call",
        }
    }
}

#[derive(Debug)]
pub(in crate::protocol::adapter) struct TranslatedTools {
    pub(in crate::protocol::adapter) upstream: Vec<Value>,
    pub(in crate::protocol::adapter) client_tools: BTreeMap<String, ClientToolTarget>,
}

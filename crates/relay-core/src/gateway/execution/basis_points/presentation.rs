//! A complete trusted catalog with a smaller, deterministic model-facing view.
use super::catalog::{client_tool_call_name, ClientTool};
use super::prepare::describe_tools;
use super::schema::{parameter_schema, prompt_schema, PROMPT_SCHEMA_NOTATION};
use crate::protocol::AdapterError;
use crate::usage::BasisPointsCatalogDiagnostics;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const OPTION: &str = "relay_bps_tool_catalog";
pub(super) const LOOKUP_TOOL: &str = "relay.catalog";
const DISCOVERY_MIN_TOOLS: usize = 24;
const DISCOVERY_MIN_CATALOG_BYTES: u64 = 24 * 1024;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum CatalogMode {
    Auto,
    Full,
    Compact,
    Discovery,
}

pub(super) fn requested_mode(request: &Value) -> Result<CatalogMode, AdapterError> {
    match request.get(OPTION).and_then(Value::as_str) {
        None if request.get(OPTION).is_none() => Ok(CatalogMode::Auto),
        Some("auto") => Ok(CatalogMode::Auto),
        Some("full") => Ok(CatalogMode::Full),
        Some("compact") => Ok(CatalogMode::Compact),
        Some("discovery") => Ok(CatalogMode::Discovery),
        _ => Err(AdapterError::invalid_request().with_parameter(OPTION)),
    }
}

pub(super) fn uses_discovery(tools: &[ClientTool], request: &Value) -> bool {
    discovery_for_refs(&tools.iter().collect::<Vec<_>>(), request)
}

fn discovery_for_refs(tools: &[&ClientTool], request: &Value) -> bool {
    if tools.is_empty() || tools.iter().any(|tool| tool.key() == LOOKUP_TOOL) {
        return false;
    }
    match requested_mode(request) {
        Ok(CatalogMode::Discovery) => true,
        Ok(CatalogMode::Auto) if tools.len() >= DISCOVERY_MIN_TOOLS => {
            let mut catalog_bytes = 0;
            tools.iter().any(|tool| {
                catalog_bytes += crate::tool_policy::serialized_bytes(&tool.spec);
                catalog_bytes >= DISCOVERY_MIN_CATALOG_BYTES
            })
        }
        _ => false,
    }
}

pub(super) fn loaded_names(tools: &[&ClientTool], request: &Value) -> BTreeSet<String> {
    let available: BTreeSet<_> = tools.iter().map(|tool| tool.key()).collect();
    request
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|item| {
            matches!(
                item["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            )
        })
        .filter_map(|item| item.as_object())
        .map(client_tool_call_name)
        .filter(|name| available.contains(name))
        .collect()
}

pub(super) fn describe_catalog(tools: &[&ClientTool], request: &Value) -> String {
    if !discovery_for_refs(tools, request) {
        return describe_tools(tools, requested_mode(request) == Ok(CatalogMode::Full));
    }
    let mut text = format!(
        "Relay-local catalog discovery (not provider-hosted tool search). All indexed tools remain available. \
         Index JSON groups tools as {{kind:{{namespace:[names]}}}}. An empty namespace means each name is complete; \
         otherwise the exact callable name is namespace + '.' + name. These are not aliases. \
         Descriptions, usage rules, and argument schemas are loaded on demand. \
         Before first using a tool whose full contract is not loaded in this conversation, load it \
         through native run_officejs with references=[\"{LOOKUP_TOOL}\"] and code containing JSON \
         {{\"names\":[\"exact.qualified.name\"]}} (up to 8 names), or {{\"query\":\"keywords\",\"limit\":5}}. \
         Query searches original names, descriptions, and schema text; use catalog-language keywords when names are unclear. \
         This read-only lookup runs inside Relay, never in the client or Excel, and does not satisfy required client tool choice. \
         After lookup, call the real client tool using its original identity. No lookup is needed for a text-only answer. \
         Catalog lookups may share a response with client calls; Relay consumes only the local lookups. Tool index:\n"
    );
    let mut index: BTreeMap<&str, BTreeMap<&str, Vec<&str>>> = BTreeMap::new();
    for tool in tools {
        index
            .entry(tool.kind.as_str())
            .or_default()
            .entry(tool.namespace.as_deref().unwrap_or_default())
            .or_default()
            .push(tool.name.as_str());
    }
    for namespaces in index.values_mut() {
        for names in namespaces.values_mut() {
            names.sort_unstable();
        }
    }
    text.push_str(&serde_json::json!(index).to_string());
    text
}

pub(super) fn loaded_contracts(tools: &[ClientTool], request: &Value) -> Option<String> {
    if !uses_discovery(tools, request) {
        return None;
    }
    let mut ordered = tools.iter().collect::<Vec<_>>();
    ordered.sort_by_cached_key(|tool| tool.key());
    let loaded = loaded_names(&ordered, request);
    let known = ordered
        .into_iter()
        .filter(|tool| loaded.contains(&tool.key()))
        .collect::<Vec<_>>();
    (!known.is_empty()).then(|| {
        let notation = if known.iter().any(|tool| parameter_schema(tool).is_some()) {
            PROMPT_SCHEMA_NOTATION
        } else {
            ""
        };
        format!(
            "{notation}Loaded contracts for client tools already used in this conversation:\n{}",
            describe_tools(&known, false)
        )
    })
}

pub(super) fn loaded_schema_bytes(tools: &[ClientTool], loaded: &BTreeSet<String>) -> u64 {
    tools
        .iter()
        .filter(|tool| loaded.contains(&tool.key()))
        .filter_map(parameter_schema)
        .map(|schema| prompt_schema(schema).len() as u64)
        .sum()
}

pub(super) fn diagnostics(tools: &[ClientTool], request: &Value) -> BasisPointsCatalogDiagnostics {
    let mut ordered = tools.iter().collect::<Vec<_>>();
    ordered.sort_by_cached_key(|tool| tool.key());
    let discovery = uses_discovery(tools, request);
    let full = requested_mode(request) == Ok(CatalogMode::Full);
    let loaded = loaded_names(&ordered, request);
    BasisPointsCatalogDiagnostics {
        mode: if discovery {
            "discovery"
        } else if full {
            "full"
        } else {
            "compact"
        }
        .to_string(),
        full_schema_bytes: tools
            .iter()
            .filter_map(parameter_schema)
            .map(crate::tool_policy::serialized_bytes)
            .sum(),
        rendered_schema_bytes: tools
            .iter()
            .filter(|tool| !discovery || loaded.contains(&tool.key()))
            .filter_map(parameter_schema)
            .map(|schema| {
                if full {
                    crate::tool_policy::serialized_bytes(schema)
                } else {
                    prompt_schema(schema).len() as u64
                }
            })
            .sum(),
        catalog_prompt_bytes: describe_catalog(&ordered, request).len() as u64,
        loaded_tool_count: if discovery { loaded.len() } else { tools.len() }.min(u16::MAX as usize)
            as u16,
        ..Default::default()
    }
}

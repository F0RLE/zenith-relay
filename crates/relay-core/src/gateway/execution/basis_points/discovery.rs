//! Bounded, request-owned catalog lookup. Never executes a client tool.
use super::catalog::{client_tools, selected_tools, ClientTool};
use super::presentation::{
    diagnostics, loaded_names, loaded_schema_bytes, uses_discovery, LOOKUP_TOOL,
};
use super::response::{invalid_tool_output, parse_transport_envelope};
use crate::protocol::AdapterError;
use crate::usage::ToolUseDiagnostics;
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(in crate::gateway) mod execution;
pub(in crate::gateway::execution) use execution::{resolve, DiscoveryExecution};

const MAX_LOOKUP_ROUNDS: u8 = 8;
const MAX_LOOKUP_TOOLS: usize = 8;
const MAX_CONTINUATION_BYTES: usize = 4 * 1024 * 1024;

pub(in crate::gateway::execution) fn capture(request: &Value, tool_use: &mut ToolUseDiagnostics) {
    if let Ok(tools) = client_tools(request) {
        tool_use.basis_points_catalog =
            Some(diagnostics(&selected_tools(request, &tools), request));
        tool_use.deferred_tool_search = false;
    }
}

pub(in crate::gateway) struct DiscoverySession {
    tools: Vec<ClientTool>,
    loaded: BTreeSet<String>,
    rendered_schema_bytes: u64,
    queries: BTreeSet<String>,
    call_ids: BTreeSet<String>,
    rounds: u8,
    continuation_bytes: usize,
    prior_messages: Vec<Value>,
}

impl DiscoverySession {
    pub(in crate::gateway::execution) fn new(
        request: &Value,
    ) -> Result<Option<Self>, AdapterError> {
        let tools = selected_tools(request, &client_tools(request)?);
        if !uses_discovery(&tools, request) {
            return Ok(None);
        }
        let loaded = loaded_names(&tools.iter().collect::<Vec<_>>(), request);
        let rendered_schema_bytes = loaded_schema_bytes(&tools, &loaded);
        let call_ids = request
            .get("input")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| {
                item.get("call_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .collect();
        Ok(Some(Self {
            tools,
            loaded,
            rendered_schema_bytes,
            queries: BTreeSet::new(),
            call_ids,
            rounds: 0,
            continuation_bytes: 0,
            prior_messages: Vec::new(),
        }))
    }

    pub(in crate::gateway) fn advance(
        &mut self,
        response: &Value,
        upstream: &mut Value,
    ) -> Result<bool, AdapterError> {
        let Some(items) = response.get("output").and_then(Value::as_array) else {
            return Err(invalid_tool_output("response.output"));
        };
        // Truncated calls are not executable. Let the terminal translator keep
        // the incomplete status, visible text, and actual usage; partial call
        // IDs or argument fragments must not replace that outcome with an error.
        if response["status"] == "incomplete" {
            return Ok(false);
        }
        let mut lookups = Vec::new();
        let mut client_calls = 0;
        let mut response_call_ids = BTreeSet::new();
        for item in items.iter().filter(|item| {
            matches!(
                item["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            )
        }) {
            let call_id = item["call_id"]
                .as_str()
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(|| invalid_tool_output("output.relay.catalog.call_id"))?;
            if !response_call_ids.insert(call_id) || self.call_ids.contains(call_id) {
                return Err(invalid_tool_output("output.relay.catalog.call_id"));
            }
            let envelope = item
                .as_object()
                .and_then(|item| parse_transport_envelope(item).ok())
                .map(|(envelope, _)| envelope);
            if envelope.as_ref().is_some_and(|envelope| {
                envelope.get("tool").and_then(Value::as_str) == Some(LOOKUP_TOOL)
            }) {
                lookups.push((item, envelope.unwrap()));
            } else {
                client_calls += 1;
            }
        }
        if lookups.is_empty() {
            return Ok(false);
        }
        if response["status"] != "completed" {
            return Err(invalid_tool_output("output.relay.catalog.incomplete"));
        }
        if (client_calls == 0 && self.rounds >= MAX_LOOKUP_ROUNDS)
            || lookups.len() > MAX_LOOKUP_TOOLS
        {
            return Err(invalid_tool_output("output.relay.catalog.limit"));
        }
        let mut outputs = Vec::new();
        let mut call_ids = BTreeSet::new();
        for (item, envelope) in lookups {
            let call_id = item["call_id"]
                .as_str()
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(|| invalid_tool_output("output.relay.catalog.call_id"))?;
            if !call_ids.insert(call_id) || !self.call_ids.insert(call_id.to_string()) {
                return Err(invalid_tool_output("output.relay.catalog.call_id"));
            }
            let item_id = item["id"]
                .as_str()
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(|| invalid_tool_output("output.relay.catalog.call_id"))?;
            let arguments = super::codec::parse_json_object(envelope.get("args"));
            let selected = arguments.as_ref().and_then(|arguments| {
                lookup(&self.tools, arguments)
                    .ok()
                    .map(|selected| (arguments, selected))
            });
            if client_calls != 0 {
                // These definitions will not be rendered or sent to another
                // generation. Validate the lookup without cloning its schemas
                // or marking unseen contracts as loaded in diagnostics.
                continue;
            }
            let (arguments, selected) = match selected {
                Some(selected) => selected,
                None => {
                    // A recognized local read has no side effects. Return its
                    // validation failure as a tool result so the model can
                    // correct the lookup within this turn's existing budget.
                    // Never repair or regenerate executable client calls.
                    outputs.push(json!({
                        "type":"function_call_output", "call_id":call_id, "id":item_id,
                        "output": json!({
                            "error":"invalid_catalog_lookup",
                            "message":"Use {\"names\":[\"exact.qualified.name\"]} for 1-8 indexed tools, or {\"query\":\"keywords\",\"limit\":5} with a limit from 1 to 8. Do not combine these forms. Function code must contain a JSON object, not another transport envelope.",
                            "remainingLookups":MAX_LOOKUP_ROUNDS.saturating_sub(self.rounds + 1)
                        }).to_string()
                    }));
                    continue;
                }
            };
            let cached = !self.queries.insert(arguments.to_string());
            self.loaded.extend(selected.iter().map(|tool| tool.key()));
            self.rendered_schema_bytes += selected
                .iter()
                .filter_map(|tool| super::schema::parameter_schema(tool))
                .map(crate::tool_policy::serialized_bytes)
                .sum::<u64>();
            let contracts = selected
                .iter()
                .map(|tool| {
                    let mut contract = Value::Object(tool.spec.clone());
                    contract["name"] = json!(tool.key());
                    contract
                })
                .collect::<Vec<_>>();
            outputs.push(json!({
                "type":"function_call_output", "call_id":call_id, "id":item_id,
                "output": json!({"tools":contracts,"cached":cached,"remainingLookups":MAX_LOOKUP_ROUNDS.saturating_sub(self.rounds + 1)}).to_string()
            }));
        }
        if client_calls != 0 {
            // Schema lookups are local, side-effect-free reads. Consume them
            // without another generation: the existing client calls must be
            // validated and returned, never replayed before their results.
            return Ok(false);
        }
        let mut continuation = items.clone();
        continuation.extend(outputs);
        // Bound only the state introduced by local discovery. The original
        // request already passed admission; charging it again made every
        // lookup in a large, valid conversation fail before continuation.
        let continuation_bytes = self.continuation_bytes.saturating_add(
            crate::gateway::request_body::retained_request_bytes(&Value::Array(
                continuation.clone(),
            )),
        );
        if continuation_bytes > MAX_CONTINUATION_BYTES {
            return Err(invalid_tool_output("output.relay.catalog.bytes"));
        }
        let input = upstream
            .get_mut("input")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| invalid_tool_output("input"))?;
        // Replay the exact native items, including opaque reasoning, to the same
        // owner. Only the local lookup results are synthesized.
        input.extend(continuation);
        self.continuation_bytes = continuation_bytes;
        self.prior_messages.extend(
            items
                .iter()
                .filter(|item| matches!(item["type"].as_str(), Some("message" | "reasoning")))
                .cloned(),
        );
        self.rounds += 1;
        if let Some(metadata) = upstream.get_mut("metadata").and_then(Value::as_object_mut) {
            let next = metadata
                .get("agent_iteration")
                .and_then(Value::as_str)
                .and_then(|v| v.parse::<u64>().ok())
                .and_then(|v| v.checked_add(1))
                .ok_or_else(|| invalid_tool_output("metadata.agent_iteration"))?;
            metadata.insert("agent_iteration".to_string(), json!(next.to_string()));
        }
        Ok(true)
    }

    pub(in crate::gateway) fn finish(&self, response: &mut Value) {
        if let Some(items) = response.get_mut("output").and_then(Value::as_array_mut) {
            items.retain(|item| {
                !item
                    .as_object()
                    .and_then(|item| parse_transport_envelope(item).ok())
                    .is_some_and(|(envelope, _)| {
                        envelope.get("tool").and_then(Value::as_str) == Some(LOOKUP_TOOL)
                    })
            });
            items.splice(0..0, self.prior_messages.iter().cloned());
        }
    }

    pub(in crate::gateway) fn record(&self, tool_use: &mut ToolUseDiagnostics) {
        if let Some(catalog) = tool_use.basis_points_catalog.as_mut() {
            catalog.discovery_rounds = self.rounds;
            catalog.loaded_tool_count = self.loaded.len().min(u16::MAX as usize) as u16;
            catalog.rendered_schema_bytes = self.rendered_schema_bytes;
        }
    }
}

fn lookup<'a>(
    tools: &'a [ClientTool],
    arguments: &Value,
) -> Result<Vec<&'a ClientTool>, AdapterError> {
    let invalid = || invalid_tool_output("output.relay.catalog.arguments");
    let arguments = arguments.as_object().ok_or_else(invalid)?;
    if let Some(names) = arguments.get("names") {
        if arguments.len() != 1 {
            return Err(invalid());
        }
        let names = names
            .as_array()
            .filter(|names| !names.is_empty() && names.len() <= MAX_LOOKUP_TOOLS)
            .ok_or_else(invalid)?;
        let mut selected = Vec::new();
        for name in names {
            let name = name.as_str().ok_or_else(invalid)?;
            let tool = tools
                .iter()
                .find(|tool| tool.key() == name)
                .ok_or_else(invalid)?;
            if !selected
                .iter()
                .any(|selected: &&ClientTool| selected.key() == name)
            {
                selected.push(tool);
            }
        }
        return Ok(selected);
    }
    if arguments
        .keys()
        .any(|key| !matches!(key.as_str(), "query" | "limit"))
    {
        return Err(invalid());
    }
    let query = arguments
        .get("query")
        .and_then(Value::as_str)
        .filter(|query| !query.trim().is_empty() && query.len() <= 500)
        .ok_or_else(invalid)?;
    let limit = match arguments.get("limit") {
        Some(value) => value
            .as_u64()
            .filter(|limit| (1..=MAX_LOOKUP_TOOLS as u64).contains(limit))
            .ok_or_else(invalid)? as usize,
        None => 5,
    };
    let terms = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|term| !term.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    let mut matches = tools
        .iter()
        .filter_map(|tool| {
            let name = tool.key().to_lowercase();
            let text = format!(
                "{} {}",
                name,
                serde_json::to_string(&tool.spec).unwrap_or_default()
            )
            .to_lowercase();
            let score = terms
                .iter()
                .map(|term| usize::from(text.contains(term)) + 3 * usize::from(name.contains(term)))
                .sum::<usize>();
            (score > 0).then_some((score, tool))
        })
        .collect::<Vec<_>>();
    matches.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.key().cmp(&right.key()))
    });
    Ok(matches
        .into_iter()
        .take(limit)
        .map(|(_, tool)| tool)
        .collect())
}

#[cfg(test)]
mod tests;

//! Stateless fan-out/fan-in for one native BPS call. Never executes client tools.
use super::catalog::{client_tool_call_name, history_tool, tool_spec, ClientTool};
use super::codec::{
    json_argument_error, json_text, parse_function_arguments, parse_json_object, parse_payload_json,
};
use super::history::fit_item_id;
use super::response::invalid_tool_output;
use super::{is_transport_tool, TRANSPORT_TOOL};
use crate::protocol::AdapterError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

pub(super) const PREFIX: &str = "relay-tool-batch-v1\n";
const ID_PREFIX: &str = "call_rb1.";
const MAX_CALLS: usize = 8;
const MAX_BYTES: usize = 1024 * 1024;
const MAX_OUTPUT_ITEMS: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Alias {
    parent: String,
    count: usize,
    index: usize,
}

impl Alias {
    fn encode(&self) -> String {
        format!(
            "{ID_PREFIX}{}.{}.{}",
            self.count,
            self.index,
            URL_SAFE_NO_PAD.encode(&self.parent)
        )
    }

    fn decode(id: &str) -> Result<Option<Self>, AdapterError> {
        let Some(tail) = id.strip_prefix(ID_PREFIX) else {
            return Ok(None);
        };
        let invalid =
            || AdapterError::invalid_request().with_parameter("input.run_officejs.batch.call_id");
        // Only the opaque native ID is carried; this is not an authorization token.
        if tail.len() > 96 {
            return Err(invalid());
        }
        let mut fields = tail.split('.');
        let count = fields
            .next()
            .and_then(|v| v.parse::<usize>().ok())
            .ok_or_else(invalid)?;
        let index = fields
            .next()
            .and_then(|v| v.parse::<usize>().ok())
            .ok_or_else(invalid)?;
        let parent = fields
            .next()
            .and_then(|v| URL_SAFE_NO_PAD.decode(v).ok())
            .and_then(|v| String::from_utf8(v).ok())
            .ok_or_else(invalid)?;
        if fields.next().is_some()
            || !(2..=MAX_CALLS).contains(&count)
            || index >= count
            || !valid_parent(&parent)
        {
            return Err(invalid());
        }
        let alias = Self {
            parent,
            count,
            index,
        };
        if alias.encode() != id {
            return Err(invalid());
        }
        Ok(Some(alias))
    }
}

fn valid_parent(id: &str) -> bool {
    !id.trim().is_empty()
        && id.len() <= 64
        && !id.starts_with(ID_PREFIX)
        && !id.chars().any(char::is_control)
}

pub(super) fn parent_call_id(id: &str) -> Option<String> {
    Alias::decode(id).ok().flatten().map(|alias| alias.parent)
}

pub(super) fn expand_output(
    items: &mut Vec<Value>,
    request: &Value,
    tools: &[ClientTool],
) -> Result<(), AdapterError> {
    if items.len() > MAX_OUTPUT_ITEMS {
        return Err(invalid_tool_output("response.output"));
    }
    let mut replacements = Vec::new();
    let mut expanded_len = items.len();
    let mut native_ids = HashSet::new();
    let history = request
        .get("input")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item["call_id"].as_str())
        .flat_map(|id| std::iter::once(id.to_owned()).chain(parent_call_id(id)))
        .collect::<HashSet<_>>();
    for (position, item) in items.iter().enumerate() {
        if !matches!(
            item["type"].as_str(),
            Some("function_call" | "custom_tool_call")
        ) {
            continue;
        }
        super::response::validate_tool_completion(item)?;
        let parent = item["call_id"]
            .as_str()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| invalid_tool_output("output.run_officejs.call_id"))?;
        if parent.starts_with(ID_PREFIX) || history.contains(parent) || !native_ids.insert(parent) {
            return Err(invalid_tool_output("output.run_officejs.call_id"));
        }
        let outer = parse_json_object(item.get("arguments"));
        let references = outer.as_ref().and_then(|v| v["references"].as_array());
        let Some(references) = references.filter(|v| v.len() > 1) else {
            continue;
        };
        if !item["name"].as_str().is_some_and(is_transport_tool) {
            return Err(invalid_tool_output("output.run_officejs.tool"));
        }
        if references.len() > MAX_CALLS {
            return Err(invalid_tool_output("output.run_officejs.batch.count"));
        }
        if !valid_parent(parent) {
            return Err(invalid_tool_output("output.run_officejs.call_id"));
        }
        if expanded_len + references.len() - 1 > MAX_OUTPUT_ITEMS {
            return Err(invalid_tool_output(
                "output.run_officejs.batch.output_limit",
            ));
        }
        if request.get("parallel_tool_calls") == Some(&Value::Bool(false)) {
            return Err(invalid_tool_output(
                "output.run_officejs.parallel_tool_calls",
            ));
        }
        let code = outer
            .as_ref()
            .and_then(|v| v["code"].as_str())
            .ok_or_else(|| invalid_tool_output("output.run_officejs.batch.code"))?;
        if code.len() > MAX_BYTES {
            return Err(invalid_tool_output("output.run_officejs.batch.size"));
        }
        let payload = code
            .strip_prefix(PREFIX)
            .or_else(|| code.strip_prefix("relay-tool-batch-v1\r\n"))
            .ok_or_else(|| invalid_tool_output("output.run_officejs.batch.prefix"))?;
        let payloads = parse_payload_json(payload).map_err(|error| {
            invalid_tool_output("output.run_officejs.batch.json")
                .with_diagnostics_from(json_argument_error(payload, &error))
        })?;
        let payloads = match payloads {
            Value::Array(payloads) => payloads,
            value => {
                return Err(invalid_tool_output("output.run_officejs.batch.array")
                    .with_shape_diagnostic(&value, None))
            }
        };
        if payloads.len() != references.len() {
            return Err(invalid_tool_output("output.run_officejs.batch.count"));
        }
        let mut children = Vec::with_capacity(references.len());
        for (index, (reference, payload)) in references.iter().zip(payloads).enumerate() {
            let name = reference
                .as_str()
                .filter(|v| {
                    !v.trim().is_empty()
                        && !is_transport_tool(v)
                        && *v != super::presentation::LOOKUP_TOOL
                })
                .ok_or_else(|| invalid_tool_output("output.run_officejs.batch.references"))?;
            let tool = tool_spec(tools, name)
                .ok_or_else(|| invalid_tool_output("output.run_officejs.tool"))?;
            let code = match payload {
                Value::String(code) => code,
                Value::Object(_) if tool.kind != "custom" => json_text(&payload)?,
                value => {
                    return Err(invalid_tool_output("output.run_officejs.batch.payload")
                        .with_shape_diagnostic(&value, None))
                }
            };
            let alias = Alias {
                parent: parent.to_owned(),
                count: references.len(),
                index,
            };
            children.push(json!({
                "type":"function_call", "name":TRANSPORT_TOOL, "call_id":alias.encode(),
                "arguments":json!({"references":[name],"code":code}).to_string()
            }));
        }
        expanded_len += children.len() - 1;
        replacements.push((position, children));
    }
    // Do not clone the whole response just to inspect batch declarations. All
    // validation above is read-only: an error still leaves the original intact.
    if replacements.is_empty() {
        return Ok(());
    }
    let mut expanded = Vec::with_capacity(expanded_len);
    let mut replacements = replacements.into_iter().peekable();
    for (position, item) in std::mem::take(items).into_iter().enumerate() {
        if replacements
            .peek()
            .is_some_and(|(index, _)| *index == position)
        {
            expanded.extend(replacements.next().unwrap().1);
        } else {
            expanded.push(item);
        }
    }
    *items = expanded;
    Ok(())
}

struct Group {
    kinds: Vec<String>,
    completed: bool,
    received: Vec<bool>,
}

/// Parallel batches may finish interleaved, but never move results across messages.
fn group_batch_results(items: Vec<Value>) -> Result<(Vec<Value>, HashSet<String>), AdapterError> {
    let is_result = |item: &Value| {
        matches!(
            item["type"].as_str(),
            Some("function_call_output" | "custom_tool_call_output")
        )
    };
    let mut output = Vec::with_capacity(items.len());
    let mut seen_batches = HashSet::new();
    let mut independent = HashSet::new();
    let mut items = items.into_iter().peekable();
    while let Some(item) = items.next() {
        if !is_result(&item)
            || !item["call_id"]
                .as_str()
                .is_some_and(|id| id.starts_with(ID_PREFIX))
        {
            output.push(item);
            continue;
        }
        let mut positions = HashMap::new();
        let mut groups = Vec::<Vec<Value>>::new();
        for result in std::iter::once(item).chain(std::iter::from_fn(|| items.next_if(is_result))) {
            let alias = result["call_id"]
                .as_str()
                .map(Alias::decode)
                .transpose()?
                .flatten();
            if let Some(alias) = alias {
                let parent = alias.parent;
                let index = *positions.entry(parent.clone()).or_insert_with(|| {
                    if !seen_batches.insert(parent.clone()) {
                        independent.insert(parent);
                    }
                    groups.push(Vec::new());
                    groups.len() - 1
                });
                groups[index].push(result);
            } else {
                groups.push(vec![result]);
            }
        }
        output.extend(groups.into_iter().flatten());
    }
    Ok((output, independent))
}

/// Definitions stay ordered and contiguous; complete result groups may interleave.
pub(super) fn collapse_history(
    items: Vec<Value>,
    tools: &[ClientTool],
) -> Result<Vec<Value>, AdapterError> {
    let invalid = || AdapterError::invalid_request().with_parameter("input.run_officejs.batch");
    let raw_ids = items
        .iter()
        .filter_map(|item| item["call_id"].as_str())
        .filter(|id| !id.starts_with(ID_PREFIX))
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    let mut groups = HashMap::<String, Group>::new();
    let mut output = Vec::with_capacity(items.len());
    let (items, independent) = group_batch_results(items)?;
    let mut items = items.into_iter();
    while let Some(item) = items.next() {
        let kind = item["type"].as_str().unwrap_or_default();
        if !matches!(
            kind,
            "function_call"
                | "custom_tool_call"
                | "function_call_output"
                | "custom_tool_call_output"
        ) {
            output.push(item);
            continue;
        }
        let Some(alias) = item["call_id"]
            .as_str()
            .map(Alias::decode)
            .transpose()?
            .flatten()
        else {
            output.push(item);
            continue;
        };
        let definitions = matches!(kind, "function_call" | "custom_tool_call");
        if definitions
            && (alias.index != 0
                || groups.contains_key(&alias.parent)
                || raw_ids.contains(&alias.parent))
        {
            return Err(invalid());
        }
        if !definitions
            && groups
                .get(&alias.parent)
                .is_none_or(|group| group.completed || group.kinds.len() != alias.count)
        {
            return Err(invalid());
        }
        if !definitions && independent.contains(&alias.parent) {
            let group = groups.get_mut(&alias.parent).ok_or_else(invalid)?;
            let valid_output = match item.get("output") {
                Some(Value::String(_)) => true,
                Some(Value::Array(parts)) => parts.iter().all(Value::is_object),
                _ => false,
            };
            if group.received[alias.index]
                || item["type"] != group.kinds[alias.index]
                || item
                    .get("status")
                    .is_some_and(|status| status != "completed")
                || !valid_output
            {
                return Err(invalid());
            }
            group.received[alias.index] = true;
            group.completed = group.received.iter().all(|received| *received);
            output.push(item);
            continue;
        }
        let mut entries = vec![item];
        for _ in 1..alias.count {
            entries.push(items.next().ok_or_else(invalid)?);
        }
        let mut ordered = vec![None; alias.count];
        for (position, entry) in entries.into_iter().enumerate() {
            let id = entry["call_id"].as_str().ok_or_else(invalid)?;
            let child = Alias::decode(id)?.ok_or_else(invalid)?;
            let entry_kind = entry["type"].as_str().unwrap_or_default();
            if child.parent != alias.parent
                || child.count != alias.count
                || ordered[child.index].is_some()
                || (definitions
                    && (child.index != position
                        || !matches!(entry_kind, "function_call" | "custom_tool_call")))
                || (!definitions
                    && !matches!(
                        entry_kind,
                        "function_call_output" | "custom_tool_call_output"
                    ))
            {
                return Err(invalid());
            }
            if entry
                .get("status")
                .is_some_and(|status| status != "completed")
            {
                return Err(invalid());
            }
            ordered[child.index] = Some(entry);
        }
        let entries = ordered
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(invalid)?;
        if definitions {
            let independent = independent.contains(&alias.parent);
            let mut references = Vec::with_capacity(alias.count);
            let mut payloads = Vec::with_capacity(alias.count);
            let mut kinds = Vec::with_capacity(alias.count);
            for entry in &entries {
                let object = entry.as_object().ok_or_else(invalid)?;
                let name = client_tool_call_name(object);
                let historic = history_tool(object);
                let tool = tool_spec(tools, &name)
                    .or(historic.as_ref())
                    .ok_or_else(invalid)?;
                if is_transport_tool(&name)
                    || name == super::presentation::LOOKUP_TOOL
                    || (entry["type"] == "custom_tool_call") != (tool.kind == "custom")
                {
                    return Err(invalid());
                }
                references.push(json!(name));
                // Objects avoid an extra escaping layer that could break bounded replay.
                payloads.push(if tool.kind == "custom" {
                    entry
                        .get("input")
                        .filter(|value| value.is_string())
                        .cloned()
                        .ok_or_else(invalid)?
                } else {
                    parse_function_arguments(object)?
                });
                kinds.push(format!("{}_output", entry["type"].as_str().unwrap()));
            }
            let code = format!("{PREFIX}{}", json_text(&Value::Array(payloads))?);
            if code.len() > MAX_BYTES {
                return Err(invalid());
            }
            if independent {
                // Replay children separately instead of moving results across instructions.
                output.extend(entries);
            } else {
                output.push(json!({
                    "type":"function_call", "name":TRANSPORT_TOOL, "status":"completed",
                    "id":fit_item_id(&format!("fc_{}", alias.parent)), "call_id":alias.parent,
                    "arguments":json!({"summary":"Run independent client tools", "extended_summary":"Relay a validated client tool batch through the BPS transport", "destructive":false, "references":references,"code":code}).to_string()
                }));
            }
            groups.insert(
                alias.parent,
                Group {
                    kinds,
                    completed: false,
                    received: if independent {
                        vec![false; alias.count]
                    } else {
                        Vec::new()
                    },
                },
            );
        } else {
            let group = groups.get_mut(&alias.parent).ok_or_else(invalid)?;
            let mut parts = Vec::new();
            for (index, mut entry) in entries.into_iter().enumerate() {
                if entry["type"] != group.kinds[index] {
                    return Err(invalid());
                }
                let object = entry.as_object_mut().ok_or_else(invalid)?;
                let result = object.remove("output").ok_or_else(invalid)?;
                parts.push(json!({"type":"input_text","text":format!("Batch result {}/{} metadata: {}",index+1,alias.count,Value::Object(object.clone()))}));
                match result {
                    Value::String(text) => parts.push(json!({"type":"input_text","text":text})),
                    Value::Array(content) if content.iter().all(Value::is_object) => {
                        parts.extend(content)
                    }
                    _ => return Err(invalid()),
                }
            }
            output
                .push(json!({"type":"function_call_output","call_id":alias.parent,"output":parts}));
            group.completed = true;
        }
    }
    if groups.values().any(|group| !group.completed) {
        return Err(invalid());
    }
    Ok(output)
}

#[cfg(test)]
mod tests;

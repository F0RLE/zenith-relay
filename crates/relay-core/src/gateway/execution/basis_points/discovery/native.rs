//! SIWC uses real native tools, not the BPS run_officejs envelope.
//! Only the read-only catalog function is executed here; client calls stay native.
use super::super::catalog::{client_tool_call_name, declared_tools};
use super::*;

pub(in crate::gateway) const NATIVE_LOOKUP: &str = "relay_catalog";

pub(super) struct NativeSettings {
    parallel: bool,
    namespaces: BTreeMap<String, serde_json::Map<String, Value>>,
}

pub(in crate::gateway::execution) fn eligible(request: &Value) -> bool {
    // A local lookup must not weaken a forced/allowed/required client choice.
    // Such requests keep their exact native catalog and selection semantics.
    if request
        .get("tool_choice")
        .is_some_and(|choice| choice != "auto")
    {
        return false;
    }
    let Ok(tools) = client_tools(request) else {
        return false;
    };
    tools.len() >= 24
        && tools
            .iter()
            .map(|tool| tool.serialized_spec.len())
            .sum::<usize>()
            >= 24 * 1024
        && !tools.iter().any(|tool| tool.name == NATIVE_LOOKUP)
}

fn tool_spec(tool: &ClientTool) -> Value {
    let mut spec = Value::Object((*tool.spec).clone());
    // Deferral is implemented locally, not by the unsupported provider search.
    spec.as_object_mut().unwrap().remove("defer_loading");
    spec
}

fn declarations(tools: Vec<&ClientTool>, settings: &NativeSettings) -> Vec<Value> {
    let mut bare = Vec::new();
    let mut groups: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for tool in tools {
        if let Some(namespace) = tool.namespace.as_deref() {
            groups.entry(namespace).or_default().push(tool_spec(tool));
        } else {
            bare.push(tool_spec(tool));
        }
    }
    for (name, tools) in groups {
        let mut group = settings.namespaces.get(name).cloned().unwrap_or_default();
        group.insert("type".into(), json!("namespace"));
        group.insert("name".into(), json!(name));
        group.insert("tools".into(), json!(tools));
        bare.push(Value::Object(group));
    }
    bare
}

fn additional(tools: Vec<Value>) -> Value {
    json!({"type":"additional_tools","role":"developer","tools":tools})
}

fn lookup_declaration() -> Value {
    json!({"type":"function","name":NATIVE_LOOKUP,
        "description":"Load available client tools by exact qualified names, or search their names, descriptions and schemas. This read-only lookup runs locally in Relay. Call the returned native tools only after their definitions have been loaded.",
        "parameters":{"type":"object","properties":{
            "names":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":8},
            "query":{"type":"string","minLength":1,"maxLength":500},
            "limit":{"type":"integer","minimum":1,"maximum":8}
        },"additionalProperties":false},"strict":false})
}

pub(in crate::gateway::execution) fn prepare_dispatch(
    original: &Value,
    upstream: &Value,
    budget: &crate::scheduler::rotation::SharedRequestBudget,
    tool_use: &mut ToolUseDiagnostics,
) -> Result<(Value, Option<DiscoverySession>), AdapterError> {
    if budget.with_budget(|budget| budget.remaining()) < 2 {
        return Ok((upstream.clone(), None));
    }
    let tools = client_tools(original)?;
    let mut settings = NativeSettings {
        parallel: original.get("parallel_tool_calls") != Some(&Value::Bool(false)),
        namespaces: BTreeMap::new(),
    };
    for value in original["input"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|item| item["type"] == "additional_tools")
        .flat_map(|item| item["tools"].as_array().into_iter().flatten())
        .chain(original["tools"].as_array().into_iter().flatten())
        .filter(|tool| tool["type"] == "namespace")
    {
        if let (Some(name), Some(spec)) = (value["name"].as_str(), value.as_object()) {
            let mut spec = spec.clone();
            spec.remove("tools");
            settings.namespaces.insert(name.to_owned(), spec);
        }
    }
    // Unsupported local schema dialects must not break native forwarding.
    if tools
        .iter()
        .any(|tool| tool.validate_parameter_schema().is_err())
    {
        return Ok((upstream.clone(), None));
    }
    let eager = loaded_names(&tools.iter().collect::<Vec<_>>(), &json!({}));
    let mut loaded = eager.clone();
    let index = super::super::presentation::catalog_index(&tools.iter().collect::<Vec<_>>());
    let instructions = format!(
        "Relay local tool catalog. All indexed tools remain available. Index groups kind, namespace and names; an empty namespace means a top-level tool. Tools already declared in this conversation are ready to call. Before calling another tool, use {NATIVE_LOOKUP} with {{\"names\":[\"exact.qualified.name\"]}} (1-8 names), or {{\"query\":\"keywords\",\"limit\":5}}. Search uses original names, descriptions and schemas. A lookup loads the original native definitions; then call the client tool normally, preserving its name, namespace and argument format. Never send a client operation to the catalog function. No lookup is needed for a text-only answer. Index: {index}"
    );
    let mut input = vec![json!({"role":"developer","content":instructions})];
    let original_input = match original.get("input") {
        Some(Value::Array(input)) => input.clone(),
        Some(Value::String(text)) => vec![json!({"role":"user","content":text})],
        _ => Vec::new(),
    };
    let mut historical = BTreeMap::new();
    let mut call_ids = BTreeSet::new();
    for item in original_input {
        if item["type"] == "additional_tools" {
            for tool in declared_tools(item.get("tools"))? {
                loaded.insert(tool.key());
                historical.insert(tool.key(), tool.serialized_spec.to_string());
            }
        }
        if matches!(
            item["type"].as_str(),
            Some("function_call" | "custom_tool_call")
        ) {
            let name = item
                .as_object()
                .map(client_tool_call_name)
                .unwrap_or_default();
            if let Some(id) = item["call_id"].as_str() {
                call_ids.insert(id.to_string());
            }
            if !loaded.contains(&name) {
                if let Some(tool) = tools.iter().find(|tool| tool.key() == name) {
                    input.push(additional(declarations(vec![tool], &settings)));
                    historical.insert(name.clone(), tool.serialized_spec.to_string());
                    loaded.insert(name);
                }
            }
        }
        input.push(item);
    }
    // Preserve explicit historical loading in place. Current declarations take
    // precedence only at the tail, without rewriting a past call's definition.
    let changed = tools
        .iter()
        .filter(|tool| {
            historical
                .get(&tool.key())
                .is_some_and(|spec| spec != tool.serialized_spec.as_ref())
        })
        .collect::<Vec<_>>();
    if !changed.is_empty() {
        input.push(additional(declarations(changed, &settings)));
    }
    let mut body = upstream.clone();
    let root = original["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|tool| {
            !matches!(
                tool["type"].as_str(),
                Some("function" | "custom" | "namespace")
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut prefix = declarations(
        tools
            .iter()
            .filter(|tool| eager.contains(&tool.key()))
            .collect(),
        &settings,
    );
    prefix.push(lookup_declaration());
    // Keep the helper/eager declarations at the same prefix on every turn.
    // Letting the ordinary root converter append them after historical tools
    // would move this block whenever the client appends another input item.
    input.insert(0, additional(prefix));
    body["tools"] = json!(root);
    body["input"] = json!(input);
    crate::providers::chatgpt::prepare_siwc_request(&mut body)?;
    let rendered_schema_bytes = loaded_schema_bytes(&tools, &loaded);
    if tool_use.client_context.is_some() {
        tool_use.basis_points_catalog = Some(crate::usage::BasisPointsCatalogDiagnostics {
            mode: "discovery".into(),
            full_schema_bytes: tools.iter().map(|tool| tool.schema_bytes(true)).sum(),
            rendered_schema_bytes,
            catalog_prompt_bytes: instructions.len() as u64,
            loaded_tool_count: loaded.len().min(u16::MAX as usize) as u16,
            ..Default::default()
        });
    }
    capture_wire_catalog(tool_use, &body);
    tool_use.filtered_tool_count = 0; // deferred tools remain discoverable, not removed
    tool_use.deferred_tool_search = false;
    let prewarm = body.pointer("/prompt_cache_options/prewarm") == Some(&Value::Bool(true));
    let session = (!prewarm).then(|| DiscoverySession {
        model: original["model"].as_str().unwrap_or_default().to_owned(),
        tools,
        loaded,
        rendered_schema_bytes: tool_use
            .client_context
            .is_some()
            .then_some(rendered_schema_bytes),
        lookup_contracts: BTreeMap::new(),
        call_ids,
        rounds: 0,
        continuation_bytes: 0,
        prior_messages: Vec::new(),
        prompt_prefix: None,
        preload_checkpoint: None,
        native: Some(settings),
    });
    Ok((body, session))
}

pub(in crate::gateway) fn capture_wire_catalog(tool_use: &mut ToolUseDiagnostics, body: &Value) {
    let stats =
        crate::tool_policy::catalog_stats_with_sizes(body, tool_use.client_context.is_some());
    tool_use.forwarded_tool_count = stats.count;
    tool_use.forwarded_schema_bytes = tool_use.client_context.is_some().then_some(stats.bytes);
}

pub(super) fn is_lookup(item: &Value) -> bool {
    item["type"] == "function_call"
        && item["name"] == NATIVE_LOOKUP
        && item.get("namespace").is_none_or(Value::is_null)
}

pub(super) fn advance(
    session: &mut DiscoverySession,
    response: &Value,
    upstream: &mut Value,
) -> Result<bool, AdapterError> {
    let items = response["output"]
        .as_array()
        .ok_or_else(|| invalid_tool_output("response.output"))?;
    if response["status"] == "incomplete" {
        return Ok(false);
    }
    if items.is_empty()
        && crate::gateway::streaming::is_empty_responses_terminal(
            &json!({"type":"response.completed","response":response}),
            false,
            0,
        )
    {
        return Err(invalid_tool_output("response.output"));
    }
    let calls = items
        .iter()
        .filter(|item| {
            matches!(
                item["type"].as_str(),
                Some("function_call" | "custom_tool_call")
            )
        })
        .collect::<Vec<_>>();
    if session
        .native
        .as_ref()
        .is_some_and(|native| !native.parallel)
        && calls.len() > 1
    {
        return Err(invalid_tool_output("output.parallel_tool_calls"));
    }
    let mut ids = BTreeSet::new();
    for item in &calls {
        validate_tool_completion(item)?;
        let id = item["call_id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| invalid_tool_output("output.call_id"))?;
        if !ids.insert(id) || session.call_ids.contains(id) {
            return Err(invalid_tool_output("output.call_id"));
        }
        if is_lookup(item) {
            continue;
        }
        let name = client_tool_call_name(item.as_object().unwrap());
        let tool = session
            .tools
            .iter()
            .find(|tool| tool.key() == name && session.loaded.contains(&name))
            .ok_or_else(|| invalid_tool_output("output.tool.name"))?;
        if item["name"].as_str() != Some(tool.name.as_str())
            || item.get("namespace").and_then(Value::as_str) != tool.namespace.as_deref()
        {
            return Err(invalid_tool_output("output.tool.namespace"));
        }
        if tool.kind == "function" && item["type"] == "function_call" {
            let args = item["arguments"]
                .as_str()
                .and_then(|args| serde_json::from_str::<Value>(args).ok())
                .filter(Value::is_object)
                .ok_or_else(|| invalid_tool_output("output.tool.arguments"))?;
            if let Some(schema) = super::super::schema::parameter_schema(tool) {
                super::super::schema::validate(&args, schema)?;
            }
        } else if tool.kind != "custom"
            || item["type"] != "custom_tool_call"
            || !item["input"].is_string()
        {
            return Err(invalid_tool_output("output.tool.type"));
        }
    }
    let lookups = calls
        .iter()
        .filter(|item| is_lookup(item))
        .copied()
        .collect::<Vec<_>>();
    if lookups.is_empty() {
        return Ok(false);
    }
    if response["status"] != "completed"
        || lookups.len() > MAX_LOOKUP_TOOLS
        || session.rounds >= MAX_LOOKUP_ROUNDS
    {
        return Err(invalid_tool_output("output.relay.catalog.limit"));
    }
    // Never execute another generation before returning already generated
    // client operations. Local lookups in a mixed response are simply hidden.
    if calls.len() != lookups.len() {
        return Ok(false);
    }
    let mut continuation = items.clone();
    let mut newly_loaded = BTreeSet::new();
    for item in lookups {
        let id = item["call_id"].as_str().unwrap();
        session.call_ids.insert(id.to_owned());
        let args = item["arguments"]
            .as_str()
            .and_then(|text| serde_json::from_str::<Value>(text).ok());
        let selected = args
            .as_ref()
            .and_then(|args| lookup(&session.tools, args).ok());
        let result = if let Some(selected) = selected {
            let names = selected.iter().map(|tool| tool.key()).collect::<Vec<_>>();
            for tool in selected {
                if session.loaded.insert(tool.key()) {
                    newly_loaded.insert(tool.key());
                }
            }
            json!({"loaded":names})
        } else {
            json!({"error":"invalid_catalog_lookup","message":"Use names (1-8 exact qualified names), or query with optional limit (1-8). Do not combine these forms."})
        };
        continuation
            .push(json!({"type":"function_call_output","call_id":id,"output":result.to_string()}));
    }
    if !newly_loaded.is_empty() {
        let namespaces = session
            .tools
            .iter()
            .filter(|tool| newly_loaded.contains(&tool.key()))
            .filter_map(|tool| tool.namespace.as_deref())
            .collect::<BTreeSet<_>>();
        let definitions = session
            .tools
            .iter()
            .filter(|tool| {
                newly_loaded.contains(&tool.key())
                    || (session.loaded.contains(&tool.key())
                        && tool
                            .namespace
                            .as_deref()
                            .is_some_and(|name| namespaces.contains(name)))
            })
            .collect();
        continuation.push(additional(declarations(
            definitions,
            session.native.as_ref().unwrap(),
        )));
    }
    let bytes = session.continuation_bytes.saturating_add(
        crate::gateway::request_body::retained_request_bytes(&json!(continuation)),
    );
    if bytes > MAX_CONTINUATION_BYTES {
        return Err(invalid_tool_output("output.relay.catalog.bytes"));
    }
    upstream["input"]
        .as_array_mut()
        .ok_or_else(|| invalid_tool_output("input"))?
        .extend(continuation);
    session.prior_messages.extend(
        items
            .iter()
            .filter(|item| {
                matches!(
                    item["type"].as_str(),
                    Some("message" | "reasoning" | "web_search_call")
                )
            })
            .cloned(),
    );
    session.rounds += 1;
    session.continuation_bytes = bytes;
    session.rendered_schema_bytes = session
        .rendered_schema_bytes
        .map(|_| loaded_schema_bytes(&session.tools, &session.loaded));
    Ok(true)
}

#[cfg(test)]
mod tests;

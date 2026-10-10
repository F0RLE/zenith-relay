use super::CODEX_TOOL_CONST_UNION_THRESHOLD;
use serde_json::{json, Map, Number, Value};

pub(super) fn normalize_account_request_common(
    request_fields: &mut Map<String, Value>,
    responses_lite: bool,
) {
    request_fields.remove("max_output_tokens");
    normalize_empty_context_management(request_fields);
    normalize_codex_tool_schemas(request_fields);
    sanitize_unstored_reasoning_items(request_fields);
    if responses_lite {
        // Codex Responses Lite accepts only complete reasoning history. Keep
        // the client-selected effort and summary settings, but always supply
        // the mandatory context mode before the request reaches either the
        // HTTP or WebSocket account transport.
        let reasoning = request_fields
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
        // Responses Lite has a stricter tool contract than the regular
        // Responses endpoint.  The upstream currently requires an explicit
        // boolean and only supports serial tool execution, even when the
        // client omitted the field.  Keep the client-owned tool definitions
        // untouched, but pin this transport-level switch to false.  This is
        // deliberately done for both OAuth and compact Lite routes so HTTP
        // and WebSocket requests cannot diverge.
        normalize_responses_lite_request(request_fields);
    }
    coerce_responses_input_array(request_fields);
}

/// Turns a string or single object `input` into a Responses item array.
/// An existing array stays unchanged. Returns whether `input` is an array.
pub(in crate::gateway) fn coerce_responses_input_array(
    request_fields: &mut Map<String, Value>,
) -> bool {
    match request_fields.get("input") {
        Some(Value::String(text)) if text.trim().is_empty() => {
            request_fields.insert("input".to_string(), Value::Array(Vec::new()));
        }
        Some(Value::String(text)) => {
            request_fields.insert(
                "input".to_string(),
                json!([{"role": "user", "content": [{"type": "input_text", "text": text}]}]),
            );
        }
        Some(Value::Object(input_item_object)) => {
            request_fields.insert(
                "input".to_string(),
                Value::Array(vec![Value::Object(input_item_object.clone())]),
            );
        }
        Some(Value::Array(_)) => {}
        _ => return false,
    }
    request_fields.get("input").is_some_and(Value::is_array)
}

/// Basis Points accepts the normal Responses body, but rejects an explicitly
/// empty context policy. Keep a non-empty policy client-owned and omit only
/// null/empty values so an absent policy remains the provider default.
pub(in crate::gateway) fn normalize_basis_points_request(request_fields: &mut Map<String, Value>) {
    normalize_empty_context_management(request_fields);
}

pub(super) fn normalize_empty_context_management(request_fields: &mut Map<String, Value>) {
    let empty = request_fields
        .get("context_management")
        .is_some_and(|context_management_value| match context_management_value {
            Value::Null => true,
            Value::Array(context_management_items) => context_management_items.is_empty(),
            Value::Object(fields) => fields.is_empty(),
            _ => false,
        });
    if empty {
        request_fields.remove("context_management");
    }
}

/// Apply the transport-level Responses Lite tool contract.
///
/// The Lite marker can arrive on a WebSocket before Relay has selected a
/// concrete route. Normalize it at request parse time as a defensive guard so
/// a route that does not use the account normalizer cannot forward
/// `parallel_tool_calls: true` alongside the Lite contract.
pub(in crate::gateway) fn normalize_responses_lite_request(
    request_fields: &mut Map<String, Value>,
) {
    if !matches!(
        request_fields.get("parallel_tool_calls"),
        Some(Value::Bool(false))
    ) {
        request_fields.insert("parallel_tool_calls".to_string(), Value::Bool(false));
    }
}

/// Codex rejects some large schemas emitted by MCP tools when an enum is
/// represented as a long `oneOf`/`anyOf` list of constant branches. Collapse
/// only branches that are provably equivalent to an enum. All other schema
/// shapes remain byte-for-byte equivalent at the JSON value level.
pub(super) fn normalize_codex_tool_schemas(request_fields: &mut Map<String, Value>) {
    let Some(tools) = request_fields
        .get_mut("tools")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for tool in tools {
        normalize_codex_tool(tool);
    }
}

fn normalize_codex_tool(tool: &mut Value) {
    let Some(tool_object) = tool.as_object_mut() else {
        return;
    };
    match tool_object.get("type").and_then(Value::as_str) {
        Some("namespace") => {
            if let Some(nested_tools) = tool_object.get_mut("tools").and_then(Value::as_array_mut) {
                for nested_tool in nested_tools {
                    normalize_codex_tool(nested_tool);
                }
            }
        }
        Some("function" | "custom") => {
            if let Some(parameters) = tool_object.get_mut("parameters") {
                normalize_codex_schema(parameters);
            }
        }
        _ => {}
    }
}

pub(super) fn normalize_codex_schema(schema_value: &mut Value) {
    super::schema::inline_local_refs(schema_value);
    normalize_codex_schema_nodes(schema_value);
}

pub(super) fn normalize_codex_schema_nodes(schema_value: &mut Value) {
    let Some(schema_object) = schema_value.as_object_mut() else {
        return;
    };

    // Visit nested object/array schemas before the current node. This covers
    // MCP schemas nested below properties/items without changing unrelated
    // tool metadata or choice constraints.
    if let Some(properties) = schema_object
        .get_mut("properties")
        .and_then(Value::as_object_mut)
    {
        for property in properties.values_mut() {
            normalize_codex_schema_nodes(property);
        }
    }
    if let Some(schema_items) = schema_object.get_mut("items") {
        normalize_codex_schema_nodes(schema_items);
    }

    let union_name = match (
        schema_object.contains_key("oneOf"),
        schema_object.contains_key("anyOf"),
    ) {
        (true, true) | (false, false) => return,
        (true, false) => "oneOf",
        (false, true) => "anyOf",
    };
    let Some(union) = schema_object.get(union_name).and_then(Value::as_array) else {
        return;
    };
    if union.len() < CODEX_TOOL_CONST_UNION_THRESHOLD {
        return;
    }

    let mut branches = Vec::with_capacity(union.len());
    let mut semantic_keys = Vec::with_capacity(union.len());
    for branch in union {
        let Some(branch_object) = branch.as_object() else {
            return;
        };
        let Some(const_value) = branch_object.get("const") else {
            return;
        };
        if branch_object
            .keys()
            .any(|key| !matches!(key.as_str(), "const" | "description" | "title"))
        {
            return;
        }
        let Some(key) = canonical_codex_scalar_key(const_value) else {
            return;
        };
        if semantic_keys.iter().any(|seen| seen == &key) {
            return;
        }
        semantic_keys.push(key);
        branches.push(const_value.clone());
    }

    if let Some(existing_enum) = schema_object.get("enum").and_then(Value::as_array) {
        let Some(existing_keys) = existing_enum
            .iter()
            .map(canonical_codex_scalar_key)
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        if !same_codex_scalar_set(&existing_keys, &semantic_keys) {
            return;
        }
        schema_object.remove(union_name);
        return;
    }

    schema_object.insert("enum".to_string(), Value::Array(branches));
    schema_object.remove(union_name);
}

pub(super) fn canonical_codex_scalar_key(scalar_value: &Value) -> Option<String> {
    match scalar_value {
        Value::String(text_value) => Some(format!("s:{text_value}")),
        Value::Number(number_value) => Some(format!("n:{}", canonical_codex_number(number_value))),
        Value::Bool(boolean_value) => Some(format!("b:{boolean_value}")),
        Value::Null => Some("null".to_string()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

pub(super) fn canonical_codex_number(number_value: &Number) -> String {
    let number_text = number_value.to_string();
    let (mantissa, exponent) = number_text
        .split_once(['e', 'E'])
        .map_or((number_text.as_str(), 0_i64), |(mantissa, exponent)| {
            (mantissa, exponent.parse::<i64>().unwrap_or(0))
        });
    let (sign, unsigned) = mantissa
        .strip_prefix('-')
        .map_or(("", mantissa), |unsigned| ("-", unsigned));
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let mut digits = format!("{whole}{fraction}");
    let first_non_zero = digits.find(|digit| digit != '0');
    let Some(first_non_zero) = first_non_zero else {
        return "0".to_string();
    };
    digits.drain(..first_non_zero);
    let mut scale = exponent - i64::try_from(fraction.len()).unwrap_or(i64::MAX);
    while digits.ends_with('0') {
        digits.pop();
        scale = scale.saturating_add(1);
    }
    format!("{sign}{digits}e{scale}")
}

pub(super) fn same_codex_scalar_set(left: &[String], right: &[String]) -> bool {
    left.len() == right.len()
        && left.len() == left.iter().collect::<std::collections::HashSet<_>>().len()
        && left.iter().all(|scalar_key| right.contains(scalar_key))
}

pub(in crate::gateway) fn responses_lite_parallel_tool_calls_valid(
    request_fields: &Map<String, Value>,
) -> bool {
    request_fields
        .get("parallel_tool_calls")
        .is_none_or(Value::is_boolean)
}

pub(super) fn sanitize_unstored_reasoning_items(request_fields: &mut Map<String, Value>) {
    let Some(input_items) = request_fields
        .get_mut("input")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for input_item in input_items {
        let Some(reasoning_item) = input_item.as_object_mut() else {
            continue;
        };
        if reasoning_item.get("type").and_then(Value::as_str) != Some("reasoning") {
            continue;
        }
        let has_encrypted_content = reasoning_item
            .get("encrypted_content")
            .and_then(Value::as_str)
            .is_some_and(|encrypted_content| !encrypted_content.trim().is_empty());
        if !has_encrypted_content {
            reasoning_item.remove("id");
            reasoning_item.remove("encrypted_content");
        }
    }
}

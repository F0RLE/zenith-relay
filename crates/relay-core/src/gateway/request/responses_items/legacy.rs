use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LegacyResponsesCallFamily {
    Function,
    Custom,
    Tool,
    Mcp,
    Computer,
}

impl LegacyResponsesCallFamily {
    fn call_type(self) -> &'static str {
        match self {
            Self::Function => "function_call",
            Self::Custom => "custom_tool_call",
            Self::Tool => "tool_call",
            Self::Mcp => "mcp_tool_call",
            Self::Computer => "computer_call",
        }
    }

    fn output_type(self) -> &'static str {
        match self {
            Self::Function => "function_call_output",
            Self::Custom => "custom_tool_call_output",
            Self::Tool => "tool_call_output",
            Self::Mcp => "mcp_tool_call_output",
            Self::Computer => "computer_call_output",
        }
    }
}

fn legacy_responses_call_family(item_type: &str) -> Option<LegacyResponsesCallFamily> {
    match item_type {
        "function_call" | "function_call_output" => Some(LegacyResponsesCallFamily::Function),
        "custom_tool_call" | "custom_tool_call_output" => Some(LegacyResponsesCallFamily::Custom),
        "tool_call" | "tool_call_output" => Some(LegacyResponsesCallFamily::Tool),
        "mcp_tool_call" | "mcp_tool_call_output" => Some(LegacyResponsesCallFamily::Mcp),
        "computer_call" | "computer_call_output" => Some(LegacyResponsesCallFamily::Computer),
        _ => None,
    }
}

/// Removes one incomplete function or custom-tool call only after the upstream
/// explicitly reports that its output is missing. The error identity must
/// match the call's item ID or call ID when the provider supplies one.
pub(in crate::gateway) fn remove_unpaired_responses_tool_call(
    request: &mut Value,
    historical_item_count: usize,
    upstream_error: &[u8],
) -> bool {
    let Some((expected_type, error_id)) = missing_responses_tool_call_identity(upstream_error)
    else {
        return false;
    };
    let Some(input_items) = request.get("input").and_then(Value::as_array) else {
        return false;
    };

    let mut incomplete = Vec::new();
    for (index, input_item) in input_items.iter().enumerate() {
        let Some(call_type) = input_item.get("type").and_then(Value::as_str) else {
            continue;
        };
        let output_type = match call_type {
            "function_call" => "function_call_output",
            "custom_tool_call" => "custom_tool_call_output",
            _ => continue,
        };
        let call_id = input_item.get("call_id").and_then(Value::as_str);
        let item_id = input_item.get("id").and_then(Value::as_str);
        let has_output = input_items
            .iter()
            .enumerate()
            .skip(index + 1)
            .any(|(_, output_item)| {
                output_item.get("type").and_then(Value::as_str) == Some(output_type)
                    && output_item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .is_some_and(|output_id| {
                            Some(output_id) == call_id || Some(output_id) == item_id
                        })
            });
        if !has_output {
            incomplete.push((index, call_type, call_id, item_id));
        }
    }

    // Multiple pending calls make it unsafe to guess which one the provider
    // rejected. Repair only a single incomplete call across the replayed turn.
    if incomplete.len() != 1 {
        return false;
    }
    let (index, call_type, call_id, item_id) = incomplete[0];
    if index >= historical_item_count
        || call_type != expected_type
        || error_id
            .as_deref()
            .is_some_and(|error_id| Some(error_id) != call_id && Some(error_id) != item_id)
    {
        return false;
    }

    request["input"]
        .as_array_mut()
        .expect("validated Responses input")
        .remove(index);
    true
}

fn missing_responses_tool_call_identity(
    error_response_body: &[u8],
) -> Option<(&'static str, Option<String>)> {
    let text = String::from_utf8_lossy(error_response_body);
    let normalized = text.to_ascii_lowercase();
    for (prefix, call_type) in [
        ("no tool output found for function call ", "function_call"),
        (
            "no tool output found for custom tool call ",
            "custom_tool_call",
        ),
        (
            "no tool output found for apply patch call ",
            "custom_tool_call",
        ),
    ] {
        let Some(start) = normalized.find(prefix) else {
            continue;
        };
        let suffix = text[start + prefix.len()..].trim_start_matches(['"', '\'', '`']);
        let call_id = suffix
            .split(|character: char| {
                character.is_whitespace()
                    || matches!(
                        character,
                        '"' | '\'' | '`' | ',' | ';' | ':' | ')' | ']' | '}' | '>'
                    )
            })
            .next()
            .unwrap_or("")
            .trim_end_matches('.')
            .trim();
        let call_id =
            (!call_id.is_empty() && call_id.len() <= 256 && !call_id.chars().any(char::is_control))
                .then(|| call_id.to_string());
        return Some((call_type, call_id));
    }
    normalized
        .contains("unanswered_function_call")
        .then_some(("function_call", None))
}

fn legacy_responses_call_id(input_item: &Value) -> Option<&str> {
    input_item
        .get("call_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|call_id| !call_id.is_empty() && call_id.len() <= 256)
}

fn next_legacy_responses_call_id(
    index: usize,
    used: &mut std::collections::HashSet<String>,
) -> String {
    let base = format!("call_missing_{index}");
    if used.insert(base.clone()) {
        return base;
    }
    for suffix in 1..=MAX_LEGACY_RESPONSES_REPAIR_ITEMS {
        let candidate = format!("{base}_{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    // The item bound makes this unreachable in practice. Keep a deterministic
    // fallback so the helper cannot spin if its limits change later.
    format!("call_missing_{index}_overflow")
}

#[derive(Clone, Debug)]
struct PendingLegacyResponsesCall {
    index: usize,
    call_id: Option<String>,
    item_id: Option<String>,
    tool_name: Option<String>,
    namespace: Option<String>,
    family: LegacyResponsesCallFamily,
}

fn legacy_responses_output_can_stand_alone(item_type: &str, tool_name: Option<&str>) -> bool {
    item_type == "function_call_output" && tool_name.is_some()
}

/// Repairs historical Responses links only after an explicit upstream rejection.
///
/// A result must identify exactly one earlier call of the same kind. Its item
/// ID may identify that call, but the result must use the call's `call_id`.
/// Plan every change before applying it: ambiguity or an anonymous orphan must
/// never delete results, cross namespaces, or leave a partially repaired turn.
pub(in crate::gateway) fn repair_legacy_responses_call_ids(request: &mut Value) -> bool {
    let Some(input_items) = request.get("input").and_then(Value::as_array) else {
        return false;
    };
    if input_items.is_empty() || input_items.len() > MAX_LEGACY_RESPONSES_REPAIR_ITEMS {
        return false;
    }

    let relevant_count = input_items
        .iter()
        .filter(|input_item| {
            input_item
                .get("type")
                .and_then(Value::as_str)
                .and_then(legacy_responses_call_family)
                .is_some()
        })
        .count();
    if relevant_count > MAX_LEGACY_RESPONSES_PENDING_CALLS {
        return false;
    }

    let mut used = std::collections::HashSet::with_capacity(relevant_count);
    for input_item in input_items {
        for field in ["call_id", "id"] {
            if let Some(item_identifier) = super::bounded_tool_call_id(input_item.get(field)) {
                used.insert(item_identifier);
            }
        }
    }

    let mut pending = Vec::<PendingLegacyResponsesCall>::with_capacity(relevant_count);
    let mut assigned = HashSet::new();
    let mut edits = Vec::new();

    for (index, input_item) in input_items.iter().enumerate() {
        let Some(input_object) = input_item.as_object() else {
            continue;
        };
        let Some(item_type) = input_object
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            continue;
        };
        let Some(family) = legacy_responses_call_family(&item_type) else {
            continue;
        };
        if ["call_id", "id", "name", "namespace"].iter().any(|field| {
            input_object.get(*field).is_some_and(|field_value| {
                !field_value.is_null()
                    && field_value.as_str().is_none_or(|field_text| {
                        field_text != field_text.trim()
                            || field_text.len() > MAX_LEGACY_RESPONSES_NAME_CHARS
                    })
            })
        }) {
            return false;
        }
        let is_call = item_type == family.call_type();
        let is_output = item_type == family.output_type();
        if !is_call && !is_output {
            continue;
        }

        let existing_id = legacy_responses_call_id(input_item).map(str::to_owned);
        let tool_name = input_object
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name_value| {
                !name_value.is_empty()
                    && name_value.chars().count() <= MAX_LEGACY_RESPONSES_NAME_CHARS
            })
            .map(str::to_string);
        let namespace = input_object
            .get("namespace")
            .and_then(Value::as_str)
            .map(str::to_owned);

        if is_call {
            if existing_id
                .as_ref()
                .is_some_and(|existing_call_id| !assigned.insert(existing_call_id.clone()))
            {
                return false;
            }
            pending.push(PendingLegacyResponsesCall {
                index,
                call_id: existing_id,
                item_id: super::bounded_tool_call_id(input_object.get("id")),
                tool_name,
                namespace,
                family,
            });
            continue;
        }

        let mut matches = pending.iter().enumerate().filter(|(_, call)| {
            call.family == family
                && tool_name
                    .as_ref()
                    .is_none_or(|tool_name| call.tool_name.as_ref() == Some(tool_name))
                && namespace
                    .as_ref()
                    .is_none_or(|namespace| call.namespace.as_ref() == Some(namespace))
                && existing_id.as_ref().is_none_or(|existing_call_id| {
                    call.call_id.as_ref() == Some(existing_call_id)
                        || call.item_id.as_ref() == Some(existing_call_id)
                })
        });
        let position = matches.next().map(|(position, _)| position);
        if matches.next().is_some() {
            return false;
        }
        let Some(position) = position else {
            if existing_id.is_some()
                || legacy_responses_output_can_stand_alone(&item_type, tool_name.as_deref())
            {
                continue;
            }
            return false;
        };
        let call = pending.remove(position);
        let call_id = call
            .call_id
            .clone()
            .or_else(|| existing_id.clone())
            .or(call.item_id)
            .unwrap_or_else(|| next_legacy_responses_call_id(call.index, &mut used));
        if call.call_id.is_none() {
            if !assigned.insert(call_id.clone()) {
                return false;
            }
            edits.push((call.index, call_id.clone()));
        }
        if existing_id.as_ref() != Some(&call_id) {
            edits.push((index, call_id));
        }
    }

    if !pending.is_empty() || edits.is_empty() {
        return false;
    }
    let input_items = request["input"].as_array_mut().expect("validated input");
    for (index, call_id) in edits {
        input_items[index]["call_id"] = Value::String(call_id);
    }
    true
}

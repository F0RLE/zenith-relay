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
    let Some(input) = request.get("input").and_then(Value::as_array) else {
        return false;
    };

    let mut incomplete = Vec::new();
    for (index, item) in input.iter().enumerate() {
        let Some(call_type) = item.get("type").and_then(Value::as_str) else {
            continue;
        };
        let output_type = match call_type {
            "function_call" => "function_call_output",
            "custom_tool_call" => "custom_tool_call_output",
            _ => continue,
        };
        let call_id = item.get("call_id").and_then(Value::as_str);
        let item_id = item.get("id").and_then(Value::as_str);
        let has_output = input.iter().enumerate().skip(index + 1).any(|(_, output)| {
            output.get("type").and_then(Value::as_str) == Some(output_type)
                && output
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

fn missing_responses_tool_call_identity(payload: &[u8]) -> Option<(&'static str, Option<String>)> {
    let text = String::from_utf8_lossy(payload);
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
        let id = suffix
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
        let id = (!id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control))
            .then(|| id.to_string());
        return Some((call_type, id));
    }
    normalized
        .contains("unanswered_function_call")
        .then_some(("function_call", None))
}

fn legacy_responses_call_id(item: &Value) -> Option<&str> {
    item.get("call_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 256)
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
    id: Option<String>,
    item_id: Option<String>,
    name: Option<String>,
    namespace: Option<String>,
    family: LegacyResponsesCallFamily,
}

fn legacy_responses_output_can_stand_alone(item_type: &str, name: Option<&str>) -> bool {
    item_type == "function_call_output" && name.is_some()
}

/// Repairs historical Responses links only after an explicit upstream rejection.
///
/// A result must identify exactly one earlier call of the same kind. Its item
/// ID may identify that call, but the result must use the call's `call_id`.
/// Plan every change before applying it: ambiguity or an anonymous orphan must
/// never delete results, cross namespaces, or leave a partially repaired turn.
pub(in crate::gateway) fn repair_legacy_responses_call_ids(request: &mut Value) -> bool {
    let Some(input) = request.get("input").and_then(Value::as_array) else {
        return false;
    };
    if input.is_empty() || input.len() > MAX_LEGACY_RESPONSES_REPAIR_ITEMS {
        return false;
    }

    let relevant_count = input
        .iter()
        .filter(|item| {
            item.get("type")
                .and_then(Value::as_str)
                .and_then(legacy_responses_call_family)
                .is_some()
        })
        .count();
    if relevant_count > MAX_LEGACY_RESPONSES_PENDING_CALLS {
        return false;
    }

    let mut used = std::collections::HashSet::with_capacity(relevant_count);
    for item in input.iter() {
        for field in ["call_id", "id"] {
            if let Some(id) = super::bounded_tool_call_id(item.get(field)) {
                used.insert(id);
            }
        }
    }

    let mut pending = Vec::<PendingLegacyResponsesCall>::with_capacity(relevant_count);
    let mut assigned = HashSet::new();
    let mut edits = Vec::new();

    for (index, item) in input.iter().enumerate() {
        let Some(object) = item.as_object() else {
            continue;
        };
        let Some(item_type) = object
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
            object.get(*field).is_some_and(|value| {
                !value.is_null()
                    && value.as_str().is_none_or(|value| {
                        value != value.trim() || value.len() > MAX_LEGACY_RESPONSES_NAME_CHARS
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

        let existing_id = legacy_responses_call_id(item).map(str::to_owned);
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| {
                !value.is_empty() && value.chars().count() <= MAX_LEGACY_RESPONSES_NAME_CHARS
            })
            .map(str::to_string);
        let namespace = object
            .get("namespace")
            .and_then(Value::as_str)
            .map(str::to_owned);

        if is_call {
            if existing_id
                .as_ref()
                .is_some_and(|id| !assigned.insert(id.clone()))
            {
                return false;
            }
            pending.push(PendingLegacyResponsesCall {
                index,
                id: existing_id,
                item_id: super::bounded_tool_call_id(object.get("id")),
                name,
                namespace,
                family,
            });
            continue;
        }

        let mut matches = pending.iter().enumerate().filter(|(_, call)| {
            call.family == family
                && name
                    .as_ref()
                    .is_none_or(|name| call.name.as_ref() == Some(name))
                && namespace
                    .as_ref()
                    .is_none_or(|namespace| call.namespace.as_ref() == Some(namespace))
                && existing_id.as_ref().is_none_or(|id| {
                    call.id.as_ref() == Some(id) || call.item_id.as_ref() == Some(id)
                })
        });
        let position = matches.next().map(|(position, _)| position);
        if matches.next().is_some() {
            return false;
        }
        let Some(position) = position else {
            if existing_id.is_some()
                || legacy_responses_output_can_stand_alone(&item_type, name.as_deref())
            {
                continue;
            }
            return false;
        };
        let call = pending.remove(position);
        let call_id = call
            .id
            .clone()
            .or_else(|| existing_id.clone())
            .or(call.item_id)
            .unwrap_or_else(|| next_legacy_responses_call_id(call.index, &mut used));
        if call.id.is_none() {
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
    let input = request["input"].as_array_mut().expect("validated input");
    for (index, call_id) in edits {
        input[index]["call_id"] = Value::String(call_id);
    }
    true
}

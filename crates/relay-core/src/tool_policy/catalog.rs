use super::CatalogStats;
use serde_json::Value;
use std::io::{self, Write};

const MAX_NAMESPACE_DEPTH: usize = 16;

fn arrays(value: &Value, f: &mut impl FnMut(&Value)) {
    for field in ["tools", "functions"] {
        if let Some(array) = value.get(field) {
            f(array);
        }
    }
    if let Some(response) = value.get("response").filter(|value| value.is_object()) {
        arrays(response, f);
    }
    if let Some(input) = value.get("input").and_then(Value::as_array) {
        for item in input {
            if item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                if let Some(array) = item.get("tools") {
                    f(array);
                }
            }
        }
    }
}

fn visit_entries(value: &Value, f: &mut impl FnMut(&Value)) {
    arrays(value, &mut |array| visit_array(array, 0, f));
}

fn visit_array(value: &Value, depth: usize, f: &mut impl FnMut(&Value)) {
    let Some(tools) = value.as_array() else {
        return;
    };
    for tool in tools {
        if depth < MAX_NAMESPACE_DEPTH
            && tool.get("type").and_then(Value::as_str) == Some("namespace")
            && tool.get("tools").is_some_and(Value::is_array)
        {
            visit_array(&tool["tools"], depth + 1, f);
        } else if depth < MAX_NAMESPACE_DEPTH
            && tool
                .get("functionDeclarations")
                .is_some_and(Value::is_array)
        {
            visit_array(&tool["functionDeclarations"], depth + 1, f);
            if tool.as_object().is_some_and(|object| object.len() > 1) {
                f(tool);
            }
        } else if tool.is_object() {
            f(tool);
        }
    }
}

pub(crate) fn catalog_stats(value: &Value) -> CatalogStats {
    let mut stats = CatalogStats::default();
    // `tool_search` is a provider control entry, not one of the client's
    // callable definitions. Do not report a catalog growing from N to N+1
    // just because Relay added that control entry.
    visit_entries(value, &mut |tool| {
        if tool.get("type").and_then(Value::as_str) != Some("tool_search") {
            stats.count = stats.count.saturating_add(1);
        }
    });
    arrays(value, &mut |array| {
        if array.is_array() {
            let mut counter = ByteCounter(0);
            if serde_json::to_writer(&mut counter, array).is_ok() {
                stats.bytes = stats.bytes.saturating_add(counter.0);
            }
        }
    });
    stats
}

struct ByteCounter(u64);

impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len() as u64);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn has_deferred_tools(request: &Value) -> bool {
    fn deferred(value: &Value, depth: usize) -> bool {
        if depth >= MAX_NAMESPACE_DEPTH {
            return true;
        }
        value.as_array().is_some_and(|tools| {
            tools.iter().any(|tool| {
                tool.get("defer_loading").and_then(Value::as_bool) == Some(true)
                    || tool
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.starts_with("tool_search"))
                    || tool
                        .get("tools")
                        .is_some_and(|tools| deferred(tools, depth + 1))
            })
        })
    }

    let mut found = false;
    arrays(request, &mut |tools| found |= deferred(tools, 0));
    found
}

/// Marks ordinary Responses function declarations for provider-native hosted
/// tool search. The function definitions stay in the request so the provider
/// can load the exact original schema later; Relay only adds the standard
/// `defer_loading` hint and the `tool_search` control tool.
pub(super) fn enable_deferred_tool_search(request: &mut Value) -> bool {
    let Some(tools) = request.get_mut("tools").and_then(Value::as_array_mut) else {
        return false;
    };
    let mut deferred_count = 0_usize;
    for tool in tools.iter_mut() {
        mark_deferred_function(tool, 0, &mut deferred_count);
    }
    if deferred_count == 0 {
        return false;
    }
    if !tools.iter().any(|tool| {
        tool.get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind == "tool_search")
    }) {
        tools.push(serde_json::json!({"type": "tool_search"}));
    }
    true
}

fn mark_deferred_function(tool: &mut Value, depth: usize, deferred_count: &mut usize) {
    if depth >= MAX_NAMESPACE_DEPTH {
        return;
    }
    if tool.get("type").and_then(Value::as_str) == Some("namespace") {
        if let Some(children) = tool.get_mut("tools").and_then(Value::as_array_mut) {
            for child in children {
                mark_deferred_function(child, depth + 1, deferred_count);
            }
        }
        return;
    }
    let Some(object) = tool.as_object_mut() else {
        return;
    };
    if object.get("type").and_then(Value::as_str) == Some("function")
        && !object.contains_key("defer_loading")
    {
        object.insert("defer_loading".to_string(), Value::Bool(true));
        *deferred_count = deferred_count.saturating_add(1);
    }
}

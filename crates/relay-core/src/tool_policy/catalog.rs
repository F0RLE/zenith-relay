use super::CatalogStats;
use serde_json::Value;
use std::io::{self, Write};

const MAX_NAMESPACE_DEPTH: usize = 16;

fn arrays(request_payload: &Value, visit_array: &mut impl FnMut(&Value)) {
    for field in ["tools", "functions"] {
        if let Some(tool_array) = request_payload.get(field) {
            visit_array(tool_array);
        }
    }
    if let Some(response_object) = request_payload
        .get("response")
        .filter(|response_value| response_value.is_object())
    {
        arrays(response_object, visit_array);
    }
    if let Some(input_items) = request_payload.get("input").and_then(Value::as_array) {
        for input_item in input_items {
            if input_item.get("type").and_then(Value::as_str) == Some("additional_tools") {
                if let Some(tool_array) = input_item.get("tools") {
                    visit_array(tool_array);
                }
            }
        }
    }
}

fn visit_entries(request_payload: &Value, visit_tool: &mut impl FnMut(&Value)) {
    arrays(request_payload, &mut |tool_array| {
        visit_array(tool_array, 0, visit_tool)
    });
}

fn visit_array(tool_array: &Value, depth: usize, visit_tool: &mut impl FnMut(&Value)) {
    let Some(tool_values) = tool_array.as_array() else {
        return;
    };
    for tool_value in tool_values {
        if depth < MAX_NAMESPACE_DEPTH
            && tool_value.get("type").and_then(Value::as_str) == Some("namespace")
            && tool_value.get("tools").is_some_and(Value::is_array)
        {
            visit_array(&tool_value["tools"], depth + 1, visit_tool);
        } else if depth < MAX_NAMESPACE_DEPTH
            && tool_value
                .get("functionDeclarations")
                .is_some_and(Value::is_array)
        {
            visit_array(&tool_value["functionDeclarations"], depth + 1, visit_tool);
            if tool_value
                .as_object()
                .is_some_and(|tool_object| tool_object.len() > 1)
            {
                visit_tool(tool_value);
            }
        } else if tool_value.is_object() {
            visit_tool(tool_value);
        }
    }
}

pub(crate) fn catalog_stats(request_payload: &Value) -> CatalogStats {
    let mut stats = CatalogStats::default();
    // `tool_search` is a provider control entry, not one of the client's
    // callable definitions. Do not report a catalog growing from N to N+1
    // just because Relay added that control entry.
    visit_entries(request_payload, &mut |tool| {
        if tool.get("type").and_then(Value::as_str) != Some("tool_search") {
            stats.count = stats.count.saturating_add(1);
        }
    });
    arrays(request_payload, &mut |tool_array| {
        if tool_array.is_array() {
            let mut counter = ByteCounter(0);
            if serde_json::to_writer(&mut counter, tool_array).is_ok() {
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

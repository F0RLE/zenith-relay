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

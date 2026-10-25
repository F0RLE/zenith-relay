use super::*;

#[test]
fn cache_maintenance_does_not_initialize_or_wait_on_busy_caches() {
    let cache = std::sync::OnceLock::new();
    assert_eq!(
        try_with_cache(&cache, |value: &mut Vec<u8>| value.len()),
        Some(0)
    );
    assert!(cache.get().is_none());
    let guard = cache
        .get_or_init(|| std::sync::Mutex::new(vec![1, 2, 3]))
        .lock()
        .unwrap();
    assert_eq!(try_with_cache(&cache, |value| value.len()), None);
    drop(guard);
    assert_eq!(try_with_cache(&cache, |value| value.len()), Some(3));
}
use serde_json::{json, Value};

mod catalog_prefix;
mod history_messages;
mod performance;
mod preparation;
mod reasoning_stream;
mod structured_output;
mod tool_identity;
mod tool_responses;

fn prompt_text(prepared: &Value) -> String {
    prepared["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["role"] == "developer")
        .filter_map(|item| item.pointer("/content/0/text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn current_catalog_text(prepared: &Value) -> &str {
    prepared["input"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .filter_map(|item| item.pointer("/content/0/text").and_then(Value::as_str))
        .find(|text| text.contains("Current client tool catalog"))
        .expect("current catalog")
}

fn request_with_tool() -> Value {
    json!({
        "model": "gpt-6-astra",
        "input": "Inspect the repo",
        "tools": [{"type":"function","name":"exec_command","description":"Run a command","parameters":{"type":"object","properties":{"cmd":{"type":"string"}},"required":["cmd"]}}]
    })
}

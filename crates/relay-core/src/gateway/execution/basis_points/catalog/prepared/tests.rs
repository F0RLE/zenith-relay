use super::*;
use serde_json::json;

#[test]
fn memory_diagnostics_expose_only_cache_accounting_not_declarations() {
    let tool = tool(
        "synthetic",
        None,
        "function",
        &declaration("synthetic-private-description"),
    );
    assert!(!tool.description(true).is_empty());
    let snapshot = cache_memory_diagnostics();
    assert_eq!(snapshot.key, "tool_presentations");
    assert_eq!(snapshot.basis, "allocation_estimate");
    assert_eq!(snapshot.limit_bytes, Some(MAX_RETAINED_BYTES));
    assert!(!serde_json::to_string(&snapshot)
        .unwrap()
        .contains("synthetic-private-description"));
}

fn declaration(description: &str) -> Map<String, Value> {
    json!({"type":"function","name":"synthetic","description":description,
        "parameters":{"type":"object","properties":{"value":{"type":"integer","minimum":1}},
        "required":["value"],"additionalProperties":false}})
    .as_object()
    .unwrap()
    .clone()
}

#[test]
fn idle_maintenance_releases_only_expired_presentations_and_keeps_live_handles() {
    let example = tool("synthetic", None, "function", &declaration("Idle fixture"));
    let mut cache = Cache::default();
    let now = Instant::now();
    for key in [0, 1] {
        cache.insert(
            [key; 32],
            Entry {
                tool: example.clone(),
                charged_bytes: 1,
                last_used: now,
            },
            now,
        );
    }
    assert!(cache.get(&[1; 32], now + IDLE_TTL / 2).is_some());
    let expired = cache.take_expired(now + IDLE_TTL);
    assert_eq!(expired.len(), 1);
    assert_eq!(cache.entries.len(), 1);
    assert!(cache.entries.contains_key(&[1; 32]));
    drop(expired);
    assert!(example.description(false).contains("Idle fixture"));
}

#[test]
fn repeated_declarations_share_parsed_and_rendered_state() {
    let spec = declaration("Synthetic reuse fixture");
    let first = tool("synthetic", Some("fixture"), "function", &spec);
    let second = tool("synthetic", Some("fixture"), "function", &spec);
    assert!(Arc::ptr_eq(&first.spec, &second.spec));
    assert!(Arc::ptr_eq(&first.presentation, &second.presentation));
    assert!(std::ptr::eq(
        first.description(false),
        second.description(false)
    ));
    assert!(first.description(false).contains("\"minimum\":1"));
    assert!(first
        .description(true)
        .contains("\"additionalProperties\":false"));
}

#[test]
fn schemas_namespaces_and_current_policy_never_leak_through_reuse() {
    let spec = declaration("Synthetic isolation fixture");
    let first = tool("synthetic", Some("first"), "function", &spec);
    let second = tool("synthetic", Some("second"), "function", &spec);
    assert!(!Arc::ptr_eq(&first.spec, &second.spec));
    assert!(second.description(false).contains("second.synthetic"));
    let mut changed = spec.clone();
    changed["parameters"]["properties"]["value"]["minimum"] = json!(9);
    let changed = tool("synthetic", Some("first"), "function", &changed);
    assert!(!Arc::ptr_eq(&first.spec, &changed.spec));
    assert!(changed.description(false).contains("\"minimum\":9"));
    let request = json!({"tools":[spec],"tool_choice":"none"});
    let catalog = super::super::client_tools(&request).unwrap();
    assert!(super::super::selected_tools(&request, &catalog).is_empty());
    assert!(super::super::client_tools(&json!({"tools":[]}))
        .unwrap()
        .is_empty());
}

#[test]
fn cache_bounds_bytes_entries_idle_time_and_oversized_definitions() {
    let mut cache = Cache::default();
    let now = Instant::now();
    let example = tool(
        "synthetic",
        None,
        "function",
        &declaration("Bounds fixture"),
    );
    let entry = |bytes| Entry {
        tool: example.clone(),
        charged_bytes: bytes,
        last_used: now,
    };
    cache.insert([255; 32], entry(MAX_ENTRY_BYTES + 1), now);
    assert!(cache.entries.is_empty());
    for index in 0..MAX_ENTRIES + 1 {
        let key: [u8; 32] = Sha256::digest(index.to_be_bytes()).into();
        cache.insert(key, entry(1), now);
    }
    assert_eq!(cache.entries.len(), MAX_ENTRIES);
    cache.entries.clear();
    for index in 0..64 {
        cache.insert([index; 32], entry(MAX_ENTRY_BYTES), now);
    }
    assert_eq!(cache.entries.len(), MAX_RETAINED_BYTES / MAX_ENTRY_BYTES);
    assert!(cache.get(&[63; 32], now + IDLE_TTL).is_none());
    cache.insert(
        [64; 32],
        Entry {
            last_used: now + IDLE_TTL,
            ..entry(1)
        },
        now + IDLE_TTL,
    );
    assert_eq!(cache.entries.len(), 1);
}

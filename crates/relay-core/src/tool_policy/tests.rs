use super::{apply_tool_policy, catalog::catalog_stats, enable_deferred_tool_search, ToolPolicy};
use crate::{ToolPolicyMode, ToolPolicyOutcome};
use serde_json::{json, Value};

fn named_catalog(count: usize) -> Value {
    json!({"tools": (0..count).map(|i| json!({
        "type":"function",
        "name":format!("tool_{i}"),
        "description":"Synthetic tool definition",
        "parameters":{"type":"object","properties":{"value":{"type":"string"}}}
    })).collect::<Vec<_>>()})
}

#[test]
fn standard_mode_is_transparent() {
    let mut request = named_catalog(73);
    let original = request.clone();
    let result = apply_tool_policy(&mut request, &ToolPolicy::default()).unwrap();

    assert_eq!(request, original);
    assert_eq!(result.before.count, 73);
    assert_eq!(result.after, result.before);
    assert_eq!(result.outcome, ToolPolicyOutcome::PassThrough);
}

#[test]
fn automatic_mode_keeps_the_catalog_and_defers_even_a_small_catalog() {
    let mut request = named_catalog(1);
    let original = request.clone();
    let policy = ToolPolicy {
        mode: ToolPolicyMode::Automatic,
    };
    let result = apply_tool_policy(&mut request, &policy).unwrap();
    assert_eq!(request, original);
    assert_eq!(result.outcome, ToolPolicyOutcome::Unchanged);

    assert!(enable_deferred_tool_search(&mut request, &policy));
    assert_eq!(request["tools"][0]["name"], "tool_0");
    assert_eq!(request["tools"][0]["defer_loading"], true);
    assert_eq!(request["tools"][1], json!({"type":"tool_search"}));
}

#[test]
fn automatic_native_search_defers_function_schemas_without_changing_identity() {
    let mut request = json!({
        "tools": [
            {"type":"namespace","name":"files","description":"File tools","tools":[
                {"type":"function","name":"read","description":"Read a file","parameters":{"type":"object"}},
                {"type":"function","name":"write","description":"Write a file","parameters":{"type":"object"}}
            ]},
            {"type":"function","name":"search","parameters":{"type":"object"}},
            {"type":"web_search"}
        ]
    });
    let policy = ToolPolicy {
        mode: ToolPolicyMode::Automatic,
    };

    assert!(enable_deferred_tool_search(&mut request, &policy));
    assert_eq!(request["tools"][0]["name"], "files");
    assert_eq!(request["tools"][0]["tools"][0]["name"], "read");
    assert_eq!(request["tools"][0]["tools"][0]["defer_loading"], true);
    assert_eq!(request["tools"][1]["name"], "search");
    assert_eq!(request["tools"][1]["defer_loading"], true);
    assert_eq!(request["tools"][3], json!({"type":"tool_search"}));
    assert_eq!(catalog_stats(&request).count, 4);
    assert!(!enable_deferred_tool_search(&mut request, &policy));
}

#[test]
fn automatic_native_search_respects_explicit_eager_function_flags() {
    let mut request = json!({"tools":[
        {"type":"function","name":"eager","defer_loading":false},
        {"type":"function","name":"deferred"}
    ]});
    let policy = ToolPolicy {
        mode: ToolPolicyMode::Automatic,
    };

    assert!(enable_deferred_tool_search(&mut request, &policy));
    assert_eq!(request["tools"][0]["defer_loading"], false);
    assert_eq!(request["tools"][1]["defer_loading"], true);
}

#[test]
fn existing_client_or_provider_deferred_catalog_is_left_untouched() {
    let mut request = json!({
        "tools": [
            {"type":"function","name":"already_deferred","defer_loading":true},
            {"type":"function","name":"plain"}
        ]
    });
    let original = request.clone();
    let policy = ToolPolicy {
        mode: ToolPolicyMode::Automatic,
    };

    assert!(!enable_deferred_tool_search(&mut request, &policy));
    assert_eq!(request, original);
}

#[test]
fn tool_search_control_entry_is_not_counted_as_a_client_tool() {
    let request = json!({
        "tools": [
            {"type":"function","name":"one","parameters":{"type":"object"}},
            {"type":"tool_search"}
        ]
    });
    let stats = catalog_stats(&request);
    assert_eq!(stats.count, 1);
    assert!(stats.bytes > 0);
}

#[test]
fn legacy_name_filtering_settings_are_read_as_standard_and_not_serialized() {
    let old: ToolPolicy = serde_json::from_value(json!({
        "mode": "allowlist",
        "enabledTools": ["read_file"],
        "disabledTools": ["delete_file"],
        "automaticToolCountThreshold": 0,
        "automaticSchemaBytesThreshold": "retired"
    }))
    .unwrap();
    assert_eq!(
        old,
        ToolPolicy {
            mode: ToolPolicyMode::PassThrough
        }
    );
    let encoded = serde_json::to_value(old).unwrap();
    assert_eq!(encoded["mode"], "pass_through");
    assert!(encoded.get("enabledTools").is_none());
    assert!(encoded.get("disabledTools").is_none());
}

#[test]
fn policy_ignores_retired_threshold_values() {
    assert!(ToolPolicy::default().normalized().is_ok());
    let legacy: ToolPolicy = serde_json::from_value(json!({
        "mode":"automatic",
        "automaticToolCountThreshold":0,
        "automaticSchemaBytesThreshold":16777217
    }))
    .unwrap();
    assert_eq!(legacy.mode, ToolPolicyMode::Automatic);
    assert!(legacy.normalized().is_ok());
}

use super::{ToolPolicy, apply_tool_policy, catalog::catalog_stats};
use crate::{ToolPolicyMode, ToolPolicyOutcome};
use serde_json::{Value, json};

fn named_catalog(count: usize) -> Value {
    json!({"tools": (0..count).map(|i| json!({
        "type":"function",
        "name":format!("tool_{i}"),
        "description":"Synthetic tool definition",
        "parameters":{"type":"object","properties":{"value":{"type":"string"}}}
    })).collect::<Vec<_>>()})
}

#[test]
fn disabled_diagnostics_skip_sizes_without_changing_the_catalog() {
    let request = named_catalog(73);
    for enabled in [false, true] {
        let forwarded = request.clone();
        let result = apply_tool_policy(&forwarded, &ToolPolicy::default(), enabled).unwrap();
        assert_eq!(forwarded, request);
        assert_eq!(result.after.count, 73);
        assert_eq!(result.context.is_some(), enabled);
        assert_eq!(result.after.bytes > 0, enabled);
    }
}

#[test]
fn standard_mode_is_transparent() {
    let request = named_catalog(73);
    let original = request.clone();
    let result = apply_tool_policy(&request, &ToolPolicy::default(), true).unwrap();

    assert_eq!(request, original);
    assert_eq!(result.before.count, 73);
    assert_eq!(result.after, result.before);
    assert_eq!(result.outcome, ToolPolicyOutcome::PassThrough);
}

#[test]
fn automatic_mode_forwards_even_a_small_catalog_unchanged() {
    let request = named_catalog(1);
    let original = request.clone();
    let policy = ToolPolicy {
        mode: ToolPolicyMode::Automatic,
    };
    let result = apply_tool_policy(&request, &policy, true).unwrap();
    assert_eq!(request, original);
    assert_eq!(result.outcome, ToolPolicyOutcome::Unchanged);
}

#[test]
fn existing_client_or_provider_deferred_catalog_is_left_untouched() {
    let request = json!({
        "tools": [
            {"type":"function","name":"already_deferred","defer_loading":true},
            {"type":"function","name":"plain"}
        ]
    });
    let original = request.clone();
    let policy = ToolPolicy {
        mode: ToolPolicyMode::Automatic,
    };

    let result = apply_tool_policy(&request, &policy, true).unwrap();
    assert_eq!(request, original);
    assert_eq!(result.outcome, ToolPolicyOutcome::Unchanged);
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
fn deferred_search_evidence_requires_both_control_and_deferred_definitions() {
    let function = json!({"type":"function","name":"fixture","defer_loading":true,"parameters":{"type":"object"}});
    let request = json!({"tools":[{"type":"tool_search"}, function]});
    let forwarded = request.clone();
    let result = apply_tool_policy(&forwarded, &ToolPolicy::default(), true).unwrap();
    assert!(result.after.deferred_search);
    assert_eq!(forwarded, request);
    assert!(!catalog_stats(&json!({"tools":[function]})).deferred_search);
    assert!(!catalog_stats(&json!({"tools":[{"type":"tool_search"}]})).deferred_search);
}

#[test]
fn legacy_name_filtering_settings_are_read_as_standard_and_not_serialized() {
    let legacy_policy: ToolPolicy = serde_json::from_value(json!({
        "mode": "allowlist",
        "enabledTools": ["read_file"],
        "disabledTools": ["delete_file"],
        "automaticToolCountThreshold": 0,
        "automaticSchemaBytesThreshold": "retired"
    }))
    .unwrap();
    assert_eq!(
        legacy_policy,
        ToolPolicy {
            mode: ToolPolicyMode::PassThrough
        }
    );
    let encoded = serde_json::to_value(legacy_policy).unwrap();
    assert_eq!(encoded["mode"], "pass_through");
    assert!(encoded.get("enabledTools").is_none());
    assert!(encoded.get("disabledTools").is_none());
}

#[test]
fn policy_ignores_retired_threshold_values() {
    let legacy: ToolPolicy = serde_json::from_value(json!({
        "mode":"automatic",
        "automaticToolCountThreshold":0,
        "automaticSchemaBytesThreshold":16777217
    }))
    .unwrap();
    assert_eq!(legacy.mode, ToolPolicyMode::Automatic);
}

#[test]
fn policy_rejects_unknown_modes_and_fields() {
    for value in [
        json!({"mode":"unsupported"}),
        json!({"mode":true}),
        json!({"mode":"pass_through","unknownSetting":true}),
    ] {
        assert!(serde_json::from_value::<ToolPolicy>(value).is_err());
    }
}

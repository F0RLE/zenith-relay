use super::*;
use crate::gateway::execution::basis_points::catalog::client_tools;
use serde_json::json;

#[test]
fn catalog_selection_reports_only_the_observed_policy() {
    use BasisPointsCatalogSelectionReason as Reason;
    for (mode, count, expected, actual_mode) in [
        ("auto", 0, Reason::NoTools, "compact"),
        ("auto", 1, Reason::BelowDiscoveryThreshold, "compact"),
        ("auto", 24, Reason::AutomaticDiscovery, "discovery"),
        ("discovery", 1, Reason::RequestedDiscovery, "discovery"),
        ("compact", 24, Reason::RequestedCompact, "compact"),
        ("full", 24, Reason::RequestedFull, "full"),
    ] {
        let tools: Vec<_> = (0..count)
            .map(|i| {
                json!({
                    "type":"function", "name":format!("synthetic_{i}"),
                    "description":"synthetic ".repeat(128), "parameters":{"type":"object"}
                })
            })
            .collect();
        let request = json!({"input":"hello","tools":tools,"relay_bps_tool_catalog":mode});
        let before = request.clone();
        let catalog = client_tools(&request).unwrap();
        let result = diagnostics(&catalog, &request);
        assert_eq!(result.selection_reason, Some(expected));
        assert_eq!(result.mode, actual_mode);
        assert_eq!(request, before);
    }
}

#[test]
fn legacy_catalog_reasons_stay_unknown_and_untrusted_reasons_are_rejected() {
    let mut legacy = serde_json::to_value(BasisPointsCatalogDiagnostics::default()).unwrap();
    assert!(legacy.get("selectionReason").is_none());
    let restored: BasisPointsCatalogDiagnostics = serde_json::from_value(legacy.clone()).unwrap();
    assert!(restored.selection_reason.is_none());
    assert!(restored.relay_tool_instruction_bytes.is_none());
    legacy["selectionReason"] = json!("private arbitrary value");
    assert!(serde_json::from_value::<BasisPointsCatalogDiagnostics>(legacy).is_err());
}

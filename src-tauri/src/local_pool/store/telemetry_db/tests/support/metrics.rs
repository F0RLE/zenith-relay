use super::super::super::support::document;
use super::diagnostic_rows;
use serde_json::{Value, json};

#[test]
fn diagnostic_report_separates_post_policy_catalog_from_prompt_embedded_contracts() {
    let mut row = diagnostic_rows().pop().unwrap();
    row.tool_use = Some(serde_json::from_value(json!({
        "clientToolCount": 312, "forwardedToolCount": 312, "forwardedSchemaBytes": 389861,
        "toolCallCount": 0, "textOutput": true, "policyMode": "pass_through",
        "upstreamContext": {"bodyBytes": 8000, "instructionBytes": 6000, "conversationBytes": 1000, "toolCatalogBytes": 0},
        "basisPointsCatalog": {"mode": "discovery", "fullSchemaBytes": 300000,
            "renderedSchemaBytes": 2075, "catalogPromptBytes": 4500, "loadedToolCount": 4, "discoveryRounds": 1}
    })).unwrap());
    let report = document(Some(&row), std::slice::from_ref(&row), false);
    let tools = &report["finalRequest"]["tools"];
    assert_eq!(tools["policyCatalog"]["toolCount"], 312);
    assert_eq!(tools["policyCatalog"]["schemaBytes"], 389861);
    assert!(tools.get("forwardedToolCount").is_none());
    assert!(tools.get("forwardedSchemaBytes").is_none());
    assert_eq!(tools["upstreamContext"]["toolCatalogBytes"], 0);
    assert_eq!(tools["catalog"]["loadedToolCount"], 4);
    assert_eq!(tools["catalog"]["renderedSchemaBytes"], 2075);
    assert_eq!(
        tools["catalog"]["representation"],
        "prompt_text_not_native_tools_array"
    );
}

#[test]
fn diagnostic_report_preserves_final_encoded_payload_without_private_content() {
    let mut row = diagnostic_rows().pop().unwrap();
    row.routing.as_mut().unwrap().transport = Some(
        serde_json::from_value(json!({
            "upstream":"http","mode":"native","requestPayload":{
                "mode":"full","fullRequestReason":"no_previous_response_id",
                "jsonBytes":100000,"bodyBytes":2000,"contentEncoding":"zstd","compressionMs":0,
                "compressionUs":420,"httpSendToHeadersMs":12
            }
        }))
        .unwrap(),
    );
    let report = document(Some(&row), std::slice::from_ref(&row), false);
    let payload = &report["finalRequest"]["routing"]["transport"]["requestPayload"];
    assert_eq!(payload["jsonBytes"], 100000);
    assert_eq!(payload["bodyBytes"], 2000);
    assert_eq!(payload["compressionMs"], 0);
    assert_eq!(payload["compressionUs"], 420);
    assert_eq!(payload["httpSendToHeadersMs"], 12);
    assert_eq!(payload["contentEncoding"], "zstd");
    assert_eq!(payload["fullRequestReason"], "no_previous_response_id");
    assert!(
        report["semantics"]["payload"]
            .as_str()
            .unwrap()
            .contains("not an acknowledgement")
    );
}

fn preparation_report(total: Option<u64>, phases: Value) -> Value {
    let mut row = diagnostic_rows().pop().unwrap();
    row.routing.as_mut().unwrap().transport = Some(
        serde_json::from_value(json!({
            "upstream": "http", "mode": "basis_points",
            "requestPreparationMs": total, "requestPreparation": phases
        }))
        .unwrap(),
    );
    document(Some(&row), std::slice::from_ref(&row), false)
}

#[test]
fn diagnostic_report_explains_preparation_remainder_without_adding_nested_validation() {
    for (phases, phase_count) in [
        (
            json!({"bodyMs":139,"schemaValidationMs":22,"catalogMs":71,"attachmentsMs":2175}),
            3,
        ),
        (
            json!({"bodyMs":139,"schemaValidationMs":22,"catalogMs":31,"serializationMs":40,"attachmentsMs":2175}),
            4,
        ),
    ] {
        let report = preparation_report(Some(2408), phases);
        let transport = &report["finalRequest"]["routing"]["transport"];
        assert_eq!(transport["requestPreparationMs"], 2408);
        assert_eq!(transport["requestPreparation"]["schemaValidationMs"], 22);
        let accounting = &transport["preparationAccounting"];
        assert_eq!(accounting["measuredNonOverlappingPhasesMs"], 2385);
        assert_eq!(accounting["unattributedMs"], 23);
        assert_eq!(accounting["observedPhaseCount"], phase_count);
        assert_eq!(accounting["status"], "consistent");
    }
}

#[test]
fn diagnostic_report_does_not_turn_missing_or_conflicting_timing_into_zero() {
    for (total, phases, status) in [
        (Some(1), json!({"bodyMs": 2}), "inconsistent"),
        (
            Some(u64::MAX),
            json!({"bodyMs": u64::MAX, "catalogMs": 1}),
            "inconsistent",
        ),
        (None, json!({"bodyMs": 2}), "not_observed"),
        (Some(22), json!({"schemaValidationMs": 22}), "not_observed"),
        (Some(22), Value::Null, "not_observed"),
    ] {
        let report = preparation_report(total, phases);
        let accounting = &report["finalRequest"]["routing"]["transport"]["preparationAccounting"];
        assert_eq!(accounting["status"], status);
        assert!(accounting["unattributedMs"].is_null());
    }
    let report = preparation_report(Some(0), json!({"bodyMs": 0}));
    let accounting = &report["finalRequest"]["routing"]["transport"]["preparationAccounting"];
    assert_eq!(accounting["status"], "consistent");
    assert_eq!(accounting["unattributedMs"], 0);
    assert_eq!(accounting["observedPhaseCount"], 1);
}

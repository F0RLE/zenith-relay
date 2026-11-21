use super::super::{
    parse_basis_points_model_access, parse_basis_points_model_access_with_diagnostics,
};
use super::*;
use serde_json::json;

#[test]
fn catalog_diagnostics_explain_each_filter_without_changing_access() {
    let body = serde_json::to_vec(&json!({
        "allowed": true,
        "model_catalog": {
            "restricted_models": ["gpt-restricted"],
            "models": [
                {"id":"gpt-future", "description":"synthetic-private-body", "unknown_option":true},
                {"id":"gpt-restricted"},
                {"id":"gpt-unavailable", "available":false},
                {"id":"gpt-disabled", "enabled":false},
                {"id":"gpt-picker", "model_picker_enabled":false},
                {"id":"gpt-policy", "policy":{"state":"denied"}},
                {"id":" GPT-FUTURE "},
                {"id":"gpt-malformed", "enabled":"synthetic-private-field"},
                {"id":"synthetic private identifier"},
                {"description":"synthetic-private-body"},
                {"id":"gpt-later", "available":false},
                {"id":"gpt-later"}
            ]
        }
    }))
    .unwrap();
    let (access, diagnostics) = parse_basis_points_model_access_with_diagnostics(&body).unwrap();
    assert_eq!(access.model_ids(), ["gpt-future", "gpt-later"]);
    assert_eq!(
        access.model_ids(),
        parse_basis_points_model_access(&body).unwrap().model_ids()
    );
    assert!(diagnostics.access_allowed);
    assert_eq!(diagnostics.received_models, 12);
    assert_eq!(diagnostics.accepted_models, 2);
    assert_eq!(diagnostics.excluded_models(), 10);
    assert_eq!(diagnostics.omitted_models(), 0);
    assert_eq!(
        diagnostics
            .models
            .iter()
            .map(|model| model.outcome)
            .collect::<Vec<_>>(),
        [
            BasisPointsModelOutcome::Accepted,
            BasisPointsModelOutcome::Restricted,
            BasisPointsModelOutcome::Unavailable,
            BasisPointsModelOutcome::Disabled,
            BasisPointsModelOutcome::PickerDisabled,
            BasisPointsModelOutcome::PolicyBlocked,
            BasisPointsModelOutcome::Duplicate,
            BasisPointsModelOutcome::MalformedRow,
            BasisPointsModelOutcome::InvalidId,
            BasisPointsModelOutcome::MalformedRow,
            BasisPointsModelOutcome::Unavailable,
            BasisPointsModelOutcome::Accepted,
        ]
    );
    assert_eq!(
        diagnostics.models[6].model_id.as_deref(),
        Some("GPT-FUTURE")
    );
    assert_eq!(
        diagnostics.models[7].model_id.as_deref(),
        Some("gpt-malformed")
    );
    assert!(diagnostics.models[8].model_id.is_none());
    assert!(diagnostics.models[9].model_id.is_none());
    let diagnostic_text = format!("{diagnostics:?}");
    for private in [
        "synthetic-private-body",
        "synthetic-private-field",
        "synthetic private identifier",
    ] {
        assert!(!diagnostic_text.contains(private));
    }
}

#[test]
fn catalog_diagnostics_bound_rows_but_count_the_complete_catalog() {
    let body = serde_json::to_vec(&json!({
        "allowed": true,
        "model_catalog": {"models": (0..96).map(|index| {
            json!({"id":format!("gpt-synthetic-{index}"), "enabled":index % 2 == 0})
        }).collect::<Vec<_>>()}
    }))
    .unwrap();
    let (access, diagnostics) = parse_basis_points_model_access_with_diagnostics(&body).unwrap();
    assert_eq!(access.models.len(), 48);
    assert_eq!(diagnostics.received_models, 96);
    assert_eq!(diagnostics.accepted_models, 48);
    assert_eq!(diagnostics.excluded_models(), 48);
    assert_eq!(diagnostics.models.len(), MAX_REPORTED_MODELS);
    assert_eq!(diagnostics.omitted_models(), 32);
    assert_eq!(diagnostics.models.last().unwrap().row, 63);
}

#[test]
fn denied_access_is_distinct_from_an_allowed_empty_catalog() {
    for allowed in [false, true] {
        let body = serde_json::to_vec(&json!({
            "allowed": allowed,
            "model_catalog": {"models": []}
        }))
        .unwrap();
        let (access, diagnostics) =
            parse_basis_points_model_access_with_diagnostics(&body).unwrap();
        assert!(access.models.is_empty());
        assert_eq!(diagnostics.access_allowed, allowed);
        assert_eq!(diagnostics.received_models, 0);
    }
    let body = br#"{"allowed":false,"model_catalog":{"models":[{"id":"gpt-denied"}]}}"#;
    let (access, diagnostics) = parse_basis_points_model_access_with_diagnostics(body).unwrap();
    assert!(access.models.is_empty());
    assert_eq!(diagnostics.received_models, 1);
    assert_eq!(diagnostics.accepted_models, 0);
    assert_eq!(diagnostics.excluded_models(), 1);
    assert_eq!(
        diagnostics.models[0].outcome,
        BasisPointsModelOutcome::AccessDenied
    );
}

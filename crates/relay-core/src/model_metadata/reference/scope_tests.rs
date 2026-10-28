use super::*;

#[test]
fn hosting_limits_never_fill_a_canonical_model_or_another_host() {
    let records = merge_reference_records(
        json!({"maker/future":{"limit":{"context":1000,"output":100}}}),
        Some(json!({
            "host-a":{"models":{"maker/future":{
                "id":"maker/future", "limit":{"input":2000,"output":500}
            }}},
            "host-b":{"models":{"alias":{
                "id":"maker/future", "limit":{"input":700,"output":50}
            }}}
        })),
        None,
        None,
    );
    assert_eq!(
        records["maker/future"]["limit"],
        json!({"context":1000,"output":100})
    );
    assert_eq!(records["host-a/maker/future"]["limit"]["input"], 2000);
    assert_eq!(records["host-b/maker/future"]["limit"]["input"], 700);
    assert_eq!(
        records["host-b/maker/future"]["aliases"],
        json!(["host-b/alias"])
    );
    assert_eq!(
        records["host-a/maker/future"]["canonical_model_id"],
        "maker/future"
    );
    let catalog = super::super::ModelMetadataCatalog::from_metadata_payload(
        &Value::Object(records.into_iter().collect()),
        None,
        None,
        false,
    )
    .unwrap();
    let origin = &catalog
        .resolve("host-b/maker/future")
        .unwrap()
        .field_provenance["capabilities.outputLimit"];
    assert_eq!(origin.provider.as_deref(), Some("host-b"));
    assert_eq!(
        origin.source_model_id.as_deref(),
        Some("host-b/maker/future")
    );
}

#[test]
fn inconsistent_and_zero_limits_are_unknown_not_clamped_or_invented() {
    let catalog = super::super::ModelMetadataCatalog::from_models_dev_json(
        r#"{"maker/future":{"limit":{"context":1000,"input":2000,"output":0}}}"#,
    )
    .unwrap();
    let limits = catalog.capabilities_for("maker/future");
    assert_eq!(limits.context_limit, Some(1000));
    assert_eq!(limits.input_limit, None);
    assert_eq!(limits.output_limit, None);
}

#[test]
fn provider_scopes_are_not_doubled_or_replaced_by_upstream_namespaces() {
    assert_eq!(provider_scoped_id("maker", "maker/future"), "maker/future");
    assert_eq!(
        provider_scoped_id("host", "maker/future"),
        "host/maker/future"
    );
    assert_eq!(provider_scoped_id("host", "future"), "host/future");
}

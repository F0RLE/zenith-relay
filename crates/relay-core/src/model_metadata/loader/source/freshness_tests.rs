use super::*;
use serde_json::json;

#[test]
fn fresh_exact_scope_wins_over_stale_primary_and_retains_gap_provenance() {
    let sources = [
        Some(json!({"maker/future":{"tool_call":false,"limit":{"context":1000,"output":100}}})),
        Some(json!({"maker":{"models":{"future":{"tool_call":true,"limit":{"output":200}}}}})),
        None,
        None,
    ];
    let payload = merge_payloads(sources.clone(), [true, false, false, false]);
    let catalog = ModelMetadataCatalog::from_metadata_payload(&payload, None, None, false).unwrap();
    let model = catalog.resolve("maker/future").unwrap();
    assert_eq!(model.capabilities.tool_call, Some(true));
    assert_eq!(model.capabilities.output_limit, Some(200));
    assert_eq!(model.capabilities.context_limit, Some(1000));
    let origin = &model.field_provenance["capabilities.outputLimit"];
    assert_eq!(origin.source, "models_dev_details");
    assert_eq!(origin.provider.as_deref(), Some("maker"));
    assert_eq!(origin.source_model_id.as_deref(), Some("maker/future"));
    assert_eq!(origin.conflicts[0].source, "models_dev");
    assert_eq!(
        model.field_provenance["capabilities.contextLimit"].source,
        "models_dev"
    );

    // A successful revalidation restores primary priority without new bytes.
    let restored = merge_payloads(sources, [false; 4]);
    assert_eq!(restored["maker/future"]["limit"]["output"], 100);
    assert_eq!(restored["maker/future"]["tool_call"], false);
}

#[test]
fn fresh_hosted_data_cannot_replace_stale_canonical_limits() {
    let payload = merge_payloads(
        [
            Some(json!({"maker/future":{"limit":{"context":1000,"output":100}}})),
            Some(
                json!({"host":{"models":{"maker/future":{"limit":{"context":2000,"output":500}}}}}),
            ),
            None,
            None,
        ],
        [true, false, false, false],
    );
    assert_eq!(payload["maker/future"]["limit"]["output"], 100);
    assert_eq!(payload["host/maker/future"]["limit"]["output"], 500);
}

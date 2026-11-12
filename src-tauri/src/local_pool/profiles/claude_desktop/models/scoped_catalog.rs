use super::tests::model;
use super::*;
use zenith_relay_core::model_metadata::ModelCapabilities;
use zenith_relay_core::{CapabilityStatus, ProtocolFeature, WireApi};

#[test]
fn desktop_catalog_omits_buffered_and_impossible_messages_routes() {
    let mut buffered = model("buffered", true, WireApi::Messages);
    buffered.protocol_routes[0]
        .features
        .insert(ProtocolFeature::Streaming, CapabilityStatus::Unsupported);
    let mut impossible = model("impossible", true, WireApi::Messages);
    impossible.protocol_routes[0].upstream_wire_api = WireApi::GeminiInteractions;
    let mut reference_denied = model("reference-denied", true, WireApi::Messages);
    reference_denied.catalog_capabilities = Some(ModelCapabilities {
        streaming: Some(false),
        ..ModelCapabilities::unknown_model()
    });
    let usable = model("usable", true, WireApi::Messages);
    let entries = inference_models(&[buffered, impossible, reference_denied, usable]).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["labelOverride"], "usable");
}

#[test]
fn desktop_cap_uses_positive_scoped_efforts_when_reference_has_no_enum() {
    let mut configured = model("opaque-model", true, WireApi::Messages);
    configured.catalog_capabilities = Some(ModelCapabilities::unknown_model());
    configured.protocol_routes[0]
        .features
        .insert(ProtocolFeature::Reasoning, CapabilityStatus::Confirmed);
    configured.protocol_routes[0].reasoning_efforts = vec!["low".into(), "high".into()];
    let original = configured.clone();
    let entries = inference_models(std::slice::from_ref(&configured)).unwrap();
    assert_eq!(entries[0]["maxEffort"], "high");
    assert_eq!(configured, original);

    configured.reasoning_configurable = true;
    configured.reasoning_allowed_levels = vec!["low".into()];
    assert_eq!(
        inference_models(std::slice::from_ref(&configured)).unwrap()[0]["maxEffort"],
        "low"
    );
    configured.reasoning_allowed_levels.clear();
    assert!(
        inference_models(&[configured]).unwrap()[0]
            .get("maxEffort")
            .is_none()
    );
}

#[test]
fn desktop_cap_cannot_revive_reference_reasoning_denials() {
    let mut configured = model("opaque-model", true, WireApi::Messages);
    configured.catalog_capabilities = Some(ModelCapabilities {
        reasoning: Some(false),
        ..ModelCapabilities::unknown_model()
    });
    configured.reasoning_allowed_levels = vec!["high".into()];
    configured.protocol_routes[0].reasoning_efforts = vec!["high".into()];
    let entries = inference_models(&[configured]).unwrap();
    assert!(entries[0].get("maxEffort").is_none());
}

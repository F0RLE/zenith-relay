use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use zenith_relay_core::{WireApi, model_metadata::ModelCapabilities, protocol::ModelSummary};

fn model(id: &str, claude_identity: bool) -> ModelSummary {
    let mut model: ModelSummary = serde_json::from_value(json!({
        "id": id, "enabled": true, "memberCount": 1,
        "inputMicroUsdPerMillion": null, "cachedInputMicroUsdPerMillion": null,
        "outputMicroUsdPerMillion": null,
        "protocolRoutes": [{"clientWireApi": WireApi::Messages, "upstreamWireApi": WireApi::Messages}]
    }))
    .unwrap();
    model.catalog_capabilities = Some(ModelCapabilities {
        reasoning: Some(true),
        reasoning_effort_levels: vec!["high".into()],
        ..ModelCapabilities::unknown_model()
    });
    model.protocol_routes[0].reasoning_efforts = vec!["high".into()];
    if claude_identity {
        model.catalog_provider = Some("anthropic".into());
        model.catalog_source_model_id = Some("anthropic/claude-opus-4-6".into());
    }
    model
}

fn owned_config(identity: bool) -> Value {
    let mut config = config("owned-model");
    if identity {
        config["inferenceModels"][0]["behavesAs"] = json!("claude-opus-4-6");
    }
    config
}

async fn project(
    fixture: &Fixture,
    models: &[ModelSummary],
    observed: Option<bool>,
) -> (Result<Vec<Value>>, usize) {
    let calls = Arc::new(AtomicUsize::new(0));
    let probe_calls = Arc::clone(&calls);
    let result =
        super::super::super::client_models_with_probe(models, &fixture.library, move || {
            probe_calls.fetch_add(1, Ordering::Relaxed);
            observed
        })
        .await;
    (result, calls.load(Ordering::Relaxed))
}

#[tokio::test]
async fn generic_catalog_skips_publisher_probe_and_preserves_profile() {
    let fixture = Fixture::new();
    let config = owned_config(false);
    apply(&fixture.library, &fixture.recovery, &config, true).unwrap();
    let profile = fs::read(fixture.profile()).unwrap();
    let recovery = fs::read(&fixture.recovery).unwrap();
    let (result, calls) = project(&fixture, &[model("generic-model", false)], None).await;
    let entries = result.unwrap();
    assert_eq!(calls, 0);
    assert_eq!(entries.len(), 1);
    assert!(entries[0].get("behavesAs").is_none());
    assert_eq!(fs::read(fixture.profile()).unwrap(), profile);
    assert_eq!(fs::read(&fixture.recovery).unwrap(), recovery);
}

#[tokio::test]
async fn empty_catalog_skips_publisher_probe_even_with_a_saved_identity() {
    let fixture = Fixture::new();
    let mut config = owned_config(true);
    apply(&fixture.library, &fixture.recovery, &config, true).unwrap();
    let mut disabled = model("disabled-claude", true);
    disabled.enabled = false;
    let (result, calls) = project(&fixture, &[disabled], None).await;
    assert_eq!(calls, 0);
    let entries = result.unwrap();
    assert!(entries.is_empty());
    config["inferenceModels"] = json!(entries);
    apply(&fixture.library, &fixture.recovery, &config, false).unwrap();
    assert_eq!(
        inspect(&fixture.library, &fixture.recovery)
            .unwrap()
            .model_count,
        0
    );
    assert!(!uses_model_identity(&fixture.library).unwrap());
    assert!(restore(&fixture.library, &fixture.recovery).unwrap());
}

#[tokio::test]
async fn eligible_identity_catalog_probes_once_and_keeps_unknown_version_protection() {
    for (observed, saved_identity) in [
        (Some(true), false),
        (Some(false), false),
        (None, false),
        (None, true),
    ] {
        let fixture = Fixture::new();
        apply(
            &fixture.library,
            &fixture.recovery,
            &owned_config(saved_identity),
            true,
        )
        .unwrap();
        let profile = fs::read(fixture.profile()).unwrap();
        let recovery = fs::read(&fixture.recovery).unwrap();
        let (result, calls) = project(&fixture, &[model("catalog-claude", true)], observed).await;
        assert_eq!(calls, 1);
        if observed.is_none() && saved_identity {
            assert_eq!(result.unwrap_err().code, ErrorCode::Conflict);
        } else {
            let entries = result.unwrap();
            assert_eq!(
                entries[0].get("behavesAs").is_some(),
                observed == Some(true)
            );
        }
        assert_eq!(fs::read(fixture.profile()).unwrap(), profile);
        assert_eq!(fs::read(&fixture.recovery).unwrap(), recovery);
    }
}

#[tokio::test]
async fn saved_identity_still_requires_publisher_evidence_after_catalog_changes() {
    for observed in [Some(true), None] {
        let fixture = Fixture::new();
        let mut config = owned_config(true);
        apply(&fixture.library, &fixture.recovery, &config, true).unwrap();
        let profile = fs::read(fixture.profile()).unwrap();
        let recovery = fs::read(&fixture.recovery).unwrap();
        let (result, calls) = project(&fixture, &[model("generic-model", false)], observed).await;
        assert_eq!(calls, 1);
        if observed.is_none() {
            assert_eq!(result.unwrap_err().code, ErrorCode::Conflict);
            assert_eq!(fs::read(fixture.profile()).unwrap(), profile);
            assert_eq!(fs::read(&fixture.recovery).unwrap(), recovery);
        } else {
            config["inferenceModels"] = json!(result.unwrap());
            apply(&fixture.library, &fixture.recovery, &config, false).unwrap();
            assert!(!uses_model_identity(&fixture.library).unwrap());
        }
    }
}

#[tokio::test]
async fn invalid_saved_profile_fails_before_spawning_metadata_helpers() {
    let fixture = Fixture::new();
    fs::write(fixture.profile(), b"invalid configuration").unwrap();
    let (result, calls) = project(&fixture, &[model("generic-model", false)], Some(true)).await;
    assert_eq!(calls, 0);
    assert_eq!(result.unwrap_err().code, ErrorCode::InvalidState);
    assert_eq!(
        fs::read(fixture.profile()).unwrap(),
        b"invalid configuration"
    );
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[tokio::test]
#[ignore = "requires Desktop 4.29.0+; reads publisher metadata without opening the client"]
async fn measure_catalog_preparation_and_publisher_probe() {
    for (label, identity) in [("generic", false), ("catalog_claude", true)] {
        let fixture = Fixture::new();
        let models: Vec<_> = (0..200)
            .map(|index| model(&format!("synthetic-model-{index}"), identity))
            .collect();
        let calls = Arc::new(AtomicUsize::new(0));
        let probe_calls = Arc::clone(&calls);
        let started = std::time::Instant::now();
        let entries =
            super::super::super::client_models_with_probe(&models, &fixture.library, move || {
                probe_calls.fetch_add(1, Ordering::Relaxed);
                crate::launcher::claude_desktop_model_identity_support()
            })
            .await
            .unwrap();
        let elapsed_us = started.elapsed().as_micros();
        let calls = calls.load(Ordering::Relaxed);
        assert_eq!(entries.len(), models.len());
        assert_eq!(calls, usize::from(identity));
        assert!(
            entries
                .iter()
                .all(|entry| entry.get("behavesAs").is_some() == identity)
        );
        assert!(!fixture.profile().exists());
        assert!(!fixture.recovery.exists());
        println!(
            "{label}: models={}, publisher_probes={calls}, elapsed_us={elapsed_us}",
            models.len()
        );
    }
}

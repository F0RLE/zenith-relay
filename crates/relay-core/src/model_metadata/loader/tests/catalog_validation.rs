use super::*;
use crate::model_metadata::{MAX_RECORDS, MAX_STRING_LENGTH};

#[test]
fn openrouter_requires_valid_identity_for_every_reference_record() {
    let valid = serde_json::json!({"id":"vendor/valid"});
    for invalid in [
        Value::Null,
        serde_json::json!(42),
        serde_json::json!({}),
        serde_json::json!({"id":42}),
        serde_json::json!({"id":""}),
        serde_json::json!({"id":" \t"}),
        serde_json::json!({"id":"x".repeat(MAX_STRING_LENGTH + 1)}),
    ] {
        for records in [
            serde_json::json!([valid, invalid]),
            serde_json::json!([invalid, valid]),
        ] {
            assert!(!valid_source_payload(
                2,
                &serde_json::json!({"data":records})
            ));
        }
    }
    assert!(valid_source_payload(
        2,
        &serde_json::json!({"data":[valid,{"id":"vendor/other","future_extension":true}]})
    ));
}

#[tokio::test]
async fn malformed_reference_record_preserves_every_source_until_valid_refresh() {
    for source in 0..4 {
        let cache = CacheDir::new();
        let server = Server::start().await;
        server.reply(
            source,
            StatusCode::OK,
            &reference_payload(source, "retained", "high").to_string(),
        );
        let loader = server.loader(&cache);
        loader.refresh(false).await.unwrap();
        let original_revision = loader.state.read().unwrap()[source]
            .envelope
            .as_ref()
            .unwrap()
            .revision
            .clone();
        let retained = reference_id(source, "retained");
        assert_eq!(loader.snapshot().reasoning_levels_for(&retained), ["high"]);

        let mut damaged = reference_payload(source, "replacement", "max");
        match source {
            0 => damaged["vendor/damaged"] = Value::Null,
            1 => damaged["vendor"]["models"]["damaged"] = Value::Null,
            2 => damaged["data"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"id":42})),
            3 => damaged["damaged"] = Value::Null,
            _ => unreachable!(),
        }
        server.reply(source, StatusCode::OK, &damaged.to_string());
        let independent = if source == 0 { 2 } else { 0 };
        server.reply(
            independent,
            StatusCode::OK,
            &reference_payload(independent, "fresh", "low").to_string(),
        );
        loader.refresh(true).await.unwrap();
        {
            let state = loader.state.read().unwrap();
            assert_eq!(
                state[source].error,
                Some(ModelMetadataError::InvalidCatalog),
                "source {source}"
            );
            assert_eq!(state[source].status, CatalogStatus::Stale);
            assert!(state[source].retry_at_ms.is_some());
            assert_eq!(
                state[source].envelope.as_ref().unwrap().revision,
                original_revision
            );
        }
        assert!(
            loader
                .snapshot()
                .resolve(&reference_id(independent, "fresh"))
                .is_some()
        );
        assert!(
            loader
                .snapshot()
                .resolve(&reference_id(source, "replacement"))
                .is_none()
        );
        let reopened = server.loader(&cache);
        assert_eq!(
            reopened.snapshot().reasoning_levels_for(&retained),
            ["high"]
        );
        assert_eq!(
            reopened.state.read().unwrap()[source].status,
            CatalogStatus::Stale
        );

        server.reply(
            source,
            StatusCode::OK,
            &reference_payload(source, "recovered", "max").to_string(),
        );
        loader.refresh(true).await.unwrap();
        assert_eq!(
            loader.state.read().unwrap()[source].status,
            CatalogStatus::Current
        );
        assert!(loader.state.read().unwrap()[source].retry_at_ms.is_none());
        assert_eq!(
            loader
                .snapshot()
                .reasoning_levels_for(&reference_id(source, "recovered")),
            ["max"]
        );
        assert!(loader.snapshot().resolve(&retained).is_none());
    }
}

fn reference_id(source: usize, model: &str) -> String {
    if source == 2 {
        format!("openrouter/vendor/{model}")
    } else {
        format!("vendor/{model}")
    }
}

fn reference_payload(source: usize, model: &str, effort: &str) -> Value {
    let scoped = format!("vendor/{model}");
    let record = serde_json::json!({"reasoning":true,"reasoning_options":[{"type":"effort","values":[effort]}]});
    match source {
        0 => serde_json::json!({scoped:record}),
        1 => serde_json::json!({"vendor":{"models":{model:record}}}),
        2 => serde_json::json!({"data":[{"id":scoped,"reasoning":{"supported_efforts":[effort]}}]}),
        3 => {
            serde_json::json!({model:{"litellm_provider":"vendor","supports_reasoning":true,"reasoning_options":[{"type":"effort","values":[effort]}]}})
        }
        _ => unreachable!(),
    }
}

#[test]
fn reference_catalogs_reject_malformed_siblings_in_each_supported_shape() {
    let valid = serde_json::json!({"id":"vendor/valid","reasoning":true});
    for (source, payload) in [
        (
            0,
            serde_json::json!({"vendor/valid":{},"vendor/invalid":null}),
        ),
        (0, serde_json::json!([valid, {"id":42}])),
        (0, serde_json::json!({"data":[valid, null]})),
        (
            1,
            serde_json::json!({"vendor":{"models":{"valid":{},"invalid":null}}}),
        ),
        (
            1,
            serde_json::json!({"vendor":{"models":{"valid":{}}},"broken":{"models":42}}),
        ),
        (
            1,
            serde_json::json!({"vendor":{"models":{"valid":{},"invalid":{"id":42}}}}),
        ),
        (
            1,
            serde_json::json!({"vendor":{"models":{"valid":{}}}," ":{"models":{"invalid":{}}}}),
        ),
        (
            3,
            serde_json::json!({"valid":{"litellm_provider":"vendor"},"invalid":null}),
        ),
        (
            3,
            serde_json::json!({"valid":{"litellm_provider":"vendor"},"invalid":{"litellm_provider":42}}),
        ),
        (
            3,
            serde_json::json!({"valid":{"litellm_provider":"vendor"},"invalid":{"litellm_provider":" "}}),
        ),
    ] {
        assert!(!valid_source_payload(source, &payload), "source {source}");
    }
}

#[test]
fn reference_catalogs_keep_supported_identity_defaults_and_extensions() {
    for (source, payload) in [
        (
            0,
            serde_json::json!({"data":[{"id":"vendor/valid","future_extension":true}],"version":2}),
        ),
        (
            0,
            serde_json::json!([{"id":"vendor/valid","future_extension":true}]),
        ),
        (
            1,
            serde_json::json!({"vendor":{"future_extension":true,"models":{"valid":{"future_extension":true}}},"empty":{"models":{}}}),
        ),
        (
            3,
            serde_json::json!({"sample_spec":{"description":"Synthetic specification"},"valid":{"litellm_provider":"vendor","future_extension":true},"alias":{"target":"valid"}}),
        ),
    ] {
        assert!(valid_source_payload(source, &payload), "source {source}");
    }
}

#[test]
fn provider_details_share_the_merged_model_count_limit() {
    let models: serde_json::Map<String, Value> = (0..MAX_RECORDS / 2)
        .map(|index| (format!("synthetic-{index}"), serde_json::json!({})))
        .collect();
    let mut payload = serde_json::json!({
        "first":{"models":models},
        "second":{"models":models}
    });
    assert!(valid_source_payload(1, &payload));
    payload["second"]["models"]["synthetic-over-limit"] = serde_json::json!({});
    assert!(!valid_source_payload(1, &payload));
}

#[tokio::test]
#[ignore = "contacts the public metadata endpoints; run explicitly"]
async fn public_reference_sources_pass_atomic_refresh_validation() {
    let cache = CacheDir::new();
    let loader = ModelMetadataCatalogLoader::open(cache.path()).unwrap();
    loader.refresh(false).await.unwrap();
    {
        let states = loader.state.read().unwrap();
        for (source, (name, state)) in SOURCES.iter().zip(states.iter()).enumerate() {
            assert_eq!(
                state.status,
                CatalogStatus::Current,
                "{name}: {:?}",
                state.error
            );
            let envelope = state.envelope.as_ref().unwrap();
            envelope
                .validate(source)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            eprintln!(
                "source={name}; revision={}; bytes={}",
                envelope.revision,
                envelope.payload.get().len()
            );
        }
    }
    let snapshot = loader.snapshot();
    assert!(!snapshot.entries.is_empty());
    let reloaded = ModelMetadataCatalogLoader::open(cache.path()).unwrap();
    assert!(
        *reloaded.snapshot() == *snapshot,
        "the complete public catalog must survive disk-cache reload"
    );
    eprintln!(
        "merged_records={}; cache_reload=unchanged",
        snapshot.entries.len()
    );
}

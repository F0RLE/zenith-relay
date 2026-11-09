use super::*;
use crate::model_metadata::MAX_RECORDS;

fn full_catalog_sources(now: u64) -> [SourceState; 4] {
    let primary: serde_json::Map<String, Value> = (0..MAX_RECORDS)
        .map(|index| (format!("canonical/{index}"), serde_json::json!({})))
        .collect();
    let mut states =
        std::array::from_fn(|_| SourceState::new(None, None, now, REFRESH_INTERVAL_MS));
    for (source, payload) in [
        (0, Value::Object(primary)),
        (2, serde_json::json!({"data":[{"id":"vendor/one-more"}]})),
    ] {
        states[source] = SourceState::new(
            Some(SourceEnvelope::new(source, payload, now).unwrap()),
            None,
            now,
            REFRESH_INTERVAL_MS,
        );
    }
    states
}

#[tokio::test]
async fn combined_capacity_failure_preserves_refresh_cache_and_rejects_partial_reload() {
    let now = crate::unix_time_ms();
    let states = full_catalog_sources(now);
    for stale in [
        [false; 4],
        [true, false, false, false],
        [false, false, true, false],
    ] {
        let mut mixed = states.clone();
        for (state, stale) in mixed.iter_mut().zip(stale) {
            if let Some(envelope) = &mut state.envelope {
                envelope.stale = stale;
            }
        }
        assert!(
            matches!(
                PreparedCatalog::new(&mixed, now, REFRESH_INTERVAL_MS),
                Err(ModelMetadataError::InvalidCatalog)
            ),
            "combined fresh/stale sources must fail instead of dropping identities"
        );
    }

    let mut overlapping = states.clone();
    overlapping[2] = SourceState::new(None, None, now, REFRESH_INTERVAL_MS);
    overlapping[1] = SourceState::new(
        Some(SourceEnvelope::new(
            1,
            serde_json::json!({"canonical":{"models":{"0":{"description":"Synthetic metadata update"}}}}),
            now,
        ).unwrap()),
        None,
        now,
        REFRESH_INTERVAL_MS,
    );
    let updated = PreparedCatalog::new(&overlapping, now, REFRESH_INTERVAL_MS).unwrap();
    assert_eq!(updated.payload.as_object().unwrap().len(), MAX_RECORDS);
    assert_eq!(
        updated.payload["canonical/0"]["description"],
        "Synthetic metadata update"
    );
    drop(updated);

    let cache = CacheDir::new();
    cache::write_states(&cache.path(), &states, "synthetic-merged-revision").unwrap();
    let saved = std::fs::read(cache.path()).unwrap();
    let server = Server::start().await;
    let reloaded = server.loader(&cache);
    assert!(reloaded.snapshot().entries.is_empty());
    {
        let loaded = reloaded.state.read().unwrap();
        for source in [0, 2] {
            assert_eq!(loaded[source].error, Some(ModelMetadataError::InvalidCache));
            assert_eq!(loaded[source].status, CatalogStatus::Stale);
            assert!(loaded[source].envelope.is_some());
            assert!(loaded[source].startup_pending);
        }
    }
    assert_eq!(std::fs::read(cache.path()).unwrap(), saved);
    reloaded.refresh(false).await.unwrap();
    assert_eq!(reloaded.status(), CatalogStatus::Current);
    assert!(reloaded.snapshot().resolve("vendor/model").is_some());

    let cache = CacheDir::new();
    let loader = server.loader(&cache);
    loader.refresh(false).await.unwrap();
    let previous = loader.snapshot();
    let saved = std::fs::read(cache.path()).unwrap();
    server.reply(
        0,
        StatusCode::OK,
        states[0].envelope.as_ref().unwrap().payload.get(),
    );
    assert_eq!(
        loader.refresh(true).await,
        Err(ModelMetadataError::InvalidCatalog)
    );
    let retained = loader.snapshot();
    assert_eq!(retained.revision, previous.revision);
    assert!(Arc::ptr_eq(&retained.entries, &previous.entries));
    assert_eq!(std::fs::read(cache.path()).unwrap(), saved);
    assert_eq!(loader.status(), CatalogStatus::Stale);
    assert!(
        loader.state.read().unwrap().iter().all(|state| {
            state.status != CatalogStatus::Updating && state.retry_at_ms.is_some()
        })
    );
    server.reply(
        0,
        StatusCode::OK,
        r#"{"vendor/recovered":{"reasoning":true}}"#,
    );
    loader.refresh(true).await.unwrap();
    assert_eq!(loader.status(), CatalogStatus::Current);
    assert!(loader.snapshot().resolve("vendor/recovered").is_some());
}

use super::*;
use serde_json::json;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::test]
async fn unchanged_and_failed_refreshes_share_data_without_changing_old_snapshots() {
    let directory = std::env::temp_dir().join(format!(
        "relay-pricing-shared-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = PricingCacheStore::new(directory.join("catalog.json"));
    let envelope = PricingCacheEnvelope::new(json!({"openai/synthetic":{"litellm_provider":"openai","input_cost_per_token":0.000001,"output_cost_per_token":0.000002}}), "revision".into(), 1).unwrap();
    store.write(&envelope).unwrap();
    let loader = PricingCatalogLoader::open(store.path()).unwrap();
    let original = loader.snapshot();
    let source = loader
        .envelope
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .payload
        .clone();
    let same = loader.catalog_for_envelope(&envelope).unwrap();
    assert!(Arc::ptr_eq(&same.entries, &original.entries));
    assert_eq!(
        loader.refresh_failed::<()>(PricingError::Network),
        Err(PricingError::Network)
    );
    assert!(!original.stale);
    let stale = loader.snapshot();
    assert!(stale.stale);
    assert!(Arc::ptr_eq(&stale.entries, &original.entries));
    assert!(Arc::ptr_eq(
        &source,
        &loader.envelope.read().unwrap().as_ref().unwrap().payload
    ));
    let response = reqwest::Response::from(
        axum::http::Response::builder()
            .status(304)
            .header("etag", "synthetic-v2")
            .body("")
            .unwrap(),
    );
    assert!(matches!(
        loader.accept_not_modified(response).await.unwrap(),
        CatalogRefreshOutcome::NotModified { .. }
    ));
    let refreshed = loader.snapshot();
    assert!(!refreshed.stale);
    assert!(stale.stale);
    assert!(Arc::ptr_eq(&refreshed.entries, &original.entries));
    assert!(Arc::ptr_eq(&refreshed.unique, &original.unique));
    assert!(Arc::ptr_eq(&refreshed.lookup, &original.lookup));
    assert_eq!(
        store.read().unwrap().unwrap().etag.as_deref(),
        Some("synthetic-v2")
    );
    let mut changed = envelope.clone();
    changed.payload["openai/synthetic"]["input_cost_per_token"] = json!(0.000003);
    changed.payload_sha256 = super::super::payload_hash(&changed.payload).unwrap();
    changed.validate().unwrap();
    let updated = loader.catalog_for_envelope(&changed).unwrap();
    assert!(!Arc::ptr_eq(&updated.entries, &original.entries));
    assert_ne!(
        updated.resolve_account("synthetic", Some("openai")).quote,
        original.resolve_account("synthetic", Some("openai")).quote
    );
    let invalid = reqwest::Response::from(
        axum::http::Response::builder()
            .status(304)
            .header("etag", "x".repeat(2048))
            .body("")
            .unwrap(),
    );
    assert_eq!(
        loader.accept_not_modified(invalid).await,
        Err(PricingError::InvalidCache)
    );
    assert_eq!(
        store.read().unwrap().unwrap().etag.as_deref(),
        Some("synthetic-v2")
    );
    let cold = PricingCatalogLoader::open(directory.join("missing.json")).unwrap();
    let response = reqwest::Response::from(
        axum::http::Response::builder()
            .status(304)
            .body("")
            .unwrap(),
    );
    assert_eq!(
        cold.accept_not_modified(response).await,
        Err(PricingError::InvalidCache)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn cache_store_round_trips_and_rejects_corrupt_data() {
    let directory = std::env::temp_dir().join(format!(
        "zenith-relay-pricing-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    let store = PricingCacheStore::new(directory.join("litellm.json"));
    let envelope = PricingCacheEnvelope::new(
        json!({"gpt-test": {"input_cost_per_token": "0.000001", "output_cost_per_token": "0.000002"}}),
        "sha256:test".into(),
        1,
    )
    .unwrap();
    store.write(&envelope).unwrap();
    assert_eq!(store.read().unwrap(), Some(envelope));
    fs::write(store.path(), b"{}").unwrap();
    assert_eq!(store.read(), Err(PricingError::InvalidCache));
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn stale_detection_is_monotonic() {
    assert!(!is_stale(100, 100, 10));
    assert!(is_stale(100, 110, 10));
    assert!(is_stale(0, 1, 10));
}

#[test]
fn cache_store_skips_an_identical_envelope() {
    let directory = std::env::temp_dir().join(format!(
        "zenith-relay-pricing-unchanged-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    let store = PricingCacheStore::new(directory.join("litellm.json"));
    let envelope = PricingCacheEnvelope::new(
        json!({
            "gpt-test": {
                "input_cost_per_token": "0.000001",
                "output_cost_per_token": "0.000002"
            }
        }),
        "sha256:test".into(),
        now_ms(),
    )
    .unwrap();
    assert!(store.write_if_changed(&envelope).unwrap());
    let modified = fs::metadata(store.path()).unwrap().modified().unwrap();
    assert!(!store.write_if_changed(&envelope).unwrap());
    assert_eq!(
        fs::metadata(store.path()).unwrap().modified().unwrap(),
        modified
    );
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn explicit_stale_marker_forces_refresh_even_with_a_fresh_timestamp() {
    let payload = json!({
        "gpt-test": {
            "input_cost_per_token": "0.000001",
            "output_cost_per_token": "0.000002"
        }
    });
    let mut envelope = PricingCacheEnvelope::new(payload, "sha256:test".into(), now_ms()).unwrap();
    envelope.stale = true;
    assert!(envelope_is_stale(
        &envelope,
        now_ms(),
        DEFAULT_CATALOG_MAX_AGE_MS
    ));
}

#[test]
fn refresh_failure_persists_stale_marker_for_the_next_startup() {
    let directory = std::env::temp_dir().join(format!(
        "zenith-relay-pricing-stale-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("litellm.json");
    let store = PricingCacheStore::new(&path);
    let envelope = PricingCacheEnvelope::new(
        json!({
            "gpt-test": {
                "litellm_provider": "openai",
                "input_cost_per_token": "0.000001",
                "output_cost_per_token": "0.000002"
            }
        }),
        "sha256:test".into(),
        now_ms(),
    )
    .unwrap();
    store.write(&envelope).unwrap();

    let loader =
        PricingCatalogLoader::open_with_max_age(&path, DEFAULT_CATALOG_MAX_AGE_MS).unwrap();
    assert_eq!(loader.status(), CatalogStatus::Current);
    assert_eq!(
        loader.refresh_failed::<()>(PricingError::Network),
        Err(PricingError::Network)
    );
    assert_eq!(loader.status(), CatalogStatus::Stale);
    assert_eq!(loader.last_error(), Some(PricingError::Network));

    let persisted = store.read().unwrap().unwrap();
    assert!(persisted.stale);
    assert!(!loader.refresh_due(now_ms()));
    assert_eq!(
        loader
            .snapshot()
            .resolve_account("gpt-test", Some("openai"))
            .quote
            .map(|price| price.input),
        Some(1_000_000)
    );

    let reopened =
        PricingCatalogLoader::open_with_max_age(&path, DEFAULT_CATALOG_MAX_AGE_MS).unwrap();
    assert_eq!(reopened.status(), CatalogStatus::Stale);
    assert!(reopened.refresh_due(now_ms()));
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn retry_backoff_progresses_and_reset_allows_an_immediate_attempt() {
    let mut retry = RefreshRetryState::default();
    let now = 10_000;

    assert!(retry.allows_attempt(now));
    retry.record_failure(now);
    assert_eq!(retry.next_retry_at_ms, Some(now + FIRST_RETRY_DELAY_MS));
    assert!(!retry.allows_attempt(now + FIRST_RETRY_DELAY_MS - 1));
    assert!(retry.allows_attempt(now + FIRST_RETRY_DELAY_MS));

    retry.record_failure(now + FIRST_RETRY_DELAY_MS);
    assert_eq!(
        retry.next_retry_at_ms,
        Some(now + FIRST_RETRY_DELAY_MS + SECOND_RETRY_DELAY_MS)
    );
    retry.record_failure(now + FIRST_RETRY_DELAY_MS + SECOND_RETRY_DELAY_MS);
    assert_eq!(
        retry.next_retry_at_ms,
        Some(now + FIRST_RETRY_DELAY_MS + SECOND_RETRY_DELAY_MS + SUBSEQUENT_RETRY_DELAY_MS)
    );

    retry.reset();
    assert_eq!(retry, RefreshRetryState::default());
    assert!(retry.allows_attempt(now));
}

#[test]
fn retry_deadline_precedes_the_daily_schedule_and_is_not_jittered() {
    let directory = std::env::temp_dir().join(format!(
        "zenith-relay-pricing-retry-deadline-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    let loader = PricingCatalogLoader::open(directory.join("litellm.json")).unwrap();
    let now = 10_000;

    loader.record_refresh_failure(now);
    assert_eq!(
        loader.next_refresh_deadline(now),
        CatalogRefreshDeadline {
            at_ms: now + FIRST_RETRY_DELAY_MS,
            kind: CatalogRefreshKind::Retry,
        }
    );
    assert!(!loader.refresh_due(now + FIRST_RETRY_DELAY_MS - 1));
    assert!(loader.refresh_due(now + FIRST_RETRY_DELAY_MS));
    let _ = fs::remove_dir_all(directory);
}

fn loader_with_fresh_test_cache(
    prefix: &str,
) -> (std::path::PathBuf, std::sync::Arc<PricingCatalogLoader>) {
    let directory = std::env::temp_dir().join(format!(
        "zenith-relay-pricing-{prefix}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("litellm.json");
    let envelope = PricingCacheEnvelope::new(
        json!({
            "gpt-test": {
                "input_cost_per_token": "0.000001",
                "output_cost_per_token": "0.000002"
            }
        }),
        "sha256:test".into(),
        now_ms(),
    )
    .unwrap();
    PricingCacheStore::new(&path).write(&envelope).unwrap();
    (
        directory,
        std::sync::Arc::new(PricingCatalogLoader::open(&path).unwrap()),
    )
}

#[tokio::test]
async fn successful_manual_refresh_wakes_schedule_waiters() {
    let (directory, loader) = loader_with_fresh_test_cache("success-notify");
    let waiter_loader = std::sync::Arc::clone(&loader);
    let waiter = tokio::spawn(async move {
        waiter_loader.wait_for_schedule_change().await;
        (
            waiter_loader.status(),
            waiter_loader.next_refresh_deadline(now_ms()),
        )
    });
    tokio::task::yield_now().await;

    loader.record_refresh_success();

    let (status, deadline) = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("successful refresh should wake the scheduler")
        .expect("schedule waiter should not panic");
    assert_eq!(status, CatalogStatus::Current);
    assert_eq!(deadline.kind, CatalogRefreshKind::Scheduled);
    assert!(deadline.at_ms > now_ms());
    let _ = fs::remove_dir_all(directory);
}

#[tokio::test]
async fn failed_manual_refresh_wakes_waiters_after_retry_state_is_recorded() {
    let (directory, loader) = loader_with_fresh_test_cache("failure-notify");
    let waiter_loader = std::sync::Arc::clone(&loader);
    let waiter = tokio::spawn(async move {
        waiter_loader.wait_for_schedule_change().await;
        (
            waiter_loader.status(),
            waiter_loader.next_refresh_deadline(now_ms()),
        )
    });
    tokio::task::yield_now().await;

    assert_eq!(
        loader.refresh_failed::<()>(PricingError::Network),
        Err(PricingError::Network)
    );

    let (status, deadline) = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("failed refresh should wake the scheduler")
        .expect("schedule waiter should not panic");
    assert_eq!(deadline.kind, CatalogRefreshKind::Retry);
    assert!(deadline.at_ms > now_ms());
    assert_eq!(status, CatalogStatus::Stale);
    let _ = fs::remove_dir_all(directory);
}

#[test]
fn startup_check_precedes_ttl_and_then_daily_schedule_controls_refresh() {
    let directory = std::env::temp_dir().join(format!(
        "zenith-relay-pricing-due-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("litellm.json");
    let payload = json!({
        "gpt-test": {
            "input_cost_per_token": "0.000001",
            "output_cost_per_token": "0.000002"
        }
    });
    let now = now_ms();
    let fresh = PricingCacheEnvelope::new(payload.clone(), "sha256:fresh".into(), now).unwrap();
    let store = PricingCacheStore::new(&path);
    store.write(&fresh).unwrap();
    let loader = PricingCatalogLoader::open_with_max_age(&path, 60_000).unwrap();
    assert!(loader.refresh_due(now));
    assert_eq!(
        loader.next_refresh_deadline(now),
        CatalogRefreshDeadline {
            at_ms: now,
            kind: CatalogRefreshKind::Startup,
        }
    );
    loader.record_refresh_success();
    assert!(!loader.refresh_due(now));
    assert_eq!(
        loader.next_refresh_deadline(now),
        CatalogRefreshDeadline {
            at_ms: now + 60_000,
            kind: CatalogRefreshKind::Scheduled,
        }
    );

    let stale =
        PricingCacheEnvelope::new(payload, "sha256:stale".into(), now.saturating_sub(61_000))
            .unwrap();
    store.write(&stale).unwrap();
    let stale_loader = PricingCatalogLoader::open_with_max_age(&path, 60_000).unwrap();
    assert!(stale_loader.refresh_due(now));
    let _ = fs::remove_dir_all(directory);
}

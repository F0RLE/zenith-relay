use super::*;
use axum::{
    extract::{Path as AxumPath, State},
    http::HeaderMap,
    response::IntoResponse,
    routing::get,
    Router,
};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct CacheDir(PathBuf);
impl CacheDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "relay-metadata-{}-{}-{}",
            std::process::id(),
            catalog_io::unix_time_ms(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> PathBuf {
        self.0.join("models-dev.json")
    }
}
impl Drop for CacheDir {
    fn drop(&mut self) {
        if let Ok(entries) = std::fs::read_dir(&self.0) {
            for entry in entries.flatten() {
                let _ = std::fs::remove_file(entry.path());
            }
        }
        let _ = std::fs::remove_dir(&self.0);
    }
}

#[derive(Clone)]
struct Reply {
    status: StatusCode,
    body: String,
}
struct MockState {
    replies: [Reply; 4],
    requests: Vec<(usize, HeaderMap)>,
}
struct Server {
    state: Arc<Mutex<MockState>>,
    task: tokio::task::JoinHandle<()>,
    urls: [String; 4],
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn start() -> Self {
        async fn handler(
            State(state): State<Arc<Mutex<MockState>>>,
            AxumPath(index): AxumPath<usize>,
            headers: HeaderMap,
        ) -> impl IntoResponse {
            let mut state = state.lock().unwrap();
            state.requests.push((index, headers));
            let reply = state.replies[index].clone();
            (
                reply.status,
                [
                    ("etag", "\"catalog-v1\""),
                    ("last-modified", "Mon, 07 Sep 2026 00:00:00 GMT"),
                ],
                reply.body,
            )
        }
        let payloads = [
            serde_json::json!({"vendor/model": {"reasoning": true}}),
            serde_json::json!({"vendor": {"models": {"model": {"id":"model", "reasoning":true, "reasoning_options":[{"type":"effort", "values":["low", "high"]}]}}}}),
            serde_json::json!({"data": [{"id":"vendor/model", "reasoning":{"supported_efforts":["high","low"]}}]}),
            serde_json::json!({"model": {"supports_low_reasoning_effort":true}}),
        ];
        let state = Arc::new(Mutex::new(MockState {
            replies: payloads.map(|p| Reply {
                status: StatusCode::OK,
                body: p.to_string(),
            }),
            requests: Vec::new(),
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = Router::new()
            .route("/{index}", get(handler))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            state,
            task,
            urls: std::array::from_fn(|i| format!("http://{address}/{i}")),
        }
    }
    fn loader(&self, cache: &CacheDir) -> ModelMetadataCatalogLoader {
        let mut loader = ModelMetadataCatalogLoader::open(cache.path()).unwrap();
        loader.urls = self.urls.clone();
        loader
    }
    fn reply(&self, index: usize, status: StatusCode, body: &str) {
        self.state.lock().unwrap().replies[index] = Reply {
            status,
            body: body.into(),
        };
    }
    fn requests(&self, index: usize) -> usize {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|(i, _)| *i == index)
            .count()
    }
}

#[tokio::test]
async fn caches_merged_payload_and_revalidates_all_sources_with_304() {
    let cache = CacheDir::new();
    let server = Server::start().await;
    let loader = server.loader(&cache);
    assert!(matches!(
        loader.refresh(false).await.unwrap(),
        CatalogRefreshOutcome::Updated { .. }
    ));
    assert_eq!(
        loader.snapshot().reasoning_levels_for("vendor/model"),
        ["low", "high"]
    );
    assert_eq!(loader.status(), CatalogStatus::Current);
    let revision = loader.snapshot().revision.clone();
    let bundle: Value = catalog_io::read_json(&cache.path(), MAX_CACHE_BYTES)
        .unwrap()
        .unwrap();
    assert_eq!(bundle["schemaVersion"], 2);
    assert_eq!(bundle["sources"].as_object().unwrap().len(), 4);
    assert_eq!(
        bundle["mergedPayload"]["vendor/model"]["reasoning_effort_levels"],
        serde_json::json!(["low", "high"])
    );
    for i in 0..4 {
        server.reply(i, StatusCode::NOT_MODIFIED, "");
    }
    let reopened = server.loader(&cache);
    assert!(matches!(
        reopened.refresh(false).await.unwrap(),
        CatalogRefreshOutcome::NotModified { .. }
    ));
    assert_eq!(reopened.snapshot().revision, revision);
    for (index, headers) in server.state.lock().unwrap().requests.iter().skip(4) {
        assert_eq!(
            headers.get(header::IF_NONE_MATCH).unwrap(),
            "\"catalog-v1\""
        );
        assert!(
            headers.contains_key(header::IF_MODIFIED_SINCE),
            "source {index}"
        );
    }
    assert_eq!(
        reopened.refresh(false).await.unwrap(),
        CatalogRefreshOutcome::Skipped
    );
}

#[tokio::test]
async fn partial_failure_preserves_stale_levels_and_refreshes_other_sources() {
    let cache = CacheDir::new();
    let server = Server::start().await;
    let loader = server.loader(&cache);
    loader.refresh(false).await.unwrap();
    server.reply(
        2,
        StatusCode::SERVICE_UNAVAILABLE,
        "private upstream body must not enter diagnostics",
    );
    server.reply(
        3,
        StatusCode::OK,
        r#"{"model":{"supports_low_reasoning_effort":true},"other":{"supportsReasoning":true}}"#,
    );
    loader.refresh(true).await.unwrap();
    assert_eq!(loader.auxiliary_status("openrouter"), CatalogStatus::Stale);
    assert_eq!(loader.auxiliary_status("litellm"), CatalogStatus::Current);
    assert_eq!(
        loader.snapshot().reasoning_levels_for("vendor/model"),
        ["low", "high"]
    );
    assert!(loader.snapshot().resolve("other").is_none());
    assert!(loader.snapshot().sources["openrouter"].stale);
    let reopened = server.loader(&cache);
    assert_eq!(
        reopened.snapshot().reasoning_levels_for("vendor/model"),
        ["low", "high"]
    );
    assert_eq!(
        reopened.auxiliary_status("openrouter"),
        CatalogStatus::Stale
    );
    assert_eq!(
        loader.refresh(false).await.unwrap(),
        CatalogRefreshOutcome::Skipped
    );
}

#[tokio::test]
async fn auxiliary_catalog_works_when_models_dev_never_loaded() {
    let cache = CacheDir::new();
    let server = Server::start().await;
    server.reply(0, StatusCode::BAD_GATEWAY, "bad");
    let loader = server.loader(&cache);
    loader.refresh(false).await.unwrap();
    assert!(loader.snapshot().resolve("vendor/model").is_none());
    assert!(server
        .loader(&cache)
        .snapshot()
        .resolve("vendor/model")
        .is_none());
    assert_eq!(loader.auxiliary_status("models_dev"), CatalogStatus::Error);
}

#[tokio::test]
async fn auxiliary_only_refresh_does_not_fetch_fresh_sources_or_change_primary_error() {
    let cache = CacheDir::new();
    let server = Server::start().await;
    let loader = server.loader(&cache);
    loader.refresh(false).await.unwrap();
    {
        let mut state = loader.state.write().unwrap();
        state[0].fail(ModelMetadataError::Network, catalog_io::unix_time_ms());
        state[2].envelope.as_mut().unwrap().fetched_at_ms = 1;
    }
    server.reply(
        2,
        StatusCode::OK,
        r#"{"data":[{"id":"vendor/model","reasoning":{"supported_efforts":["max"]}}]}"#,
    );
    let deadline = loader.next_refresh_deadline(catalog_io::unix_time_ms());
    assert_eq!(deadline.kind, CatalogRefreshKind::Scheduled);
    loader.refresh(false).await.unwrap();
    assert_eq!(
        [
            server.requests(0),
            server.requests(1),
            server.requests(2),
            server.requests(3),
        ],
        [1, 1, 2, 1]
    );
    assert_eq!(loader.auxiliary_status("models_dev"), CatalogStatus::Stale);
    assert_eq!(
        loader.snapshot().reasoning_levels_for("vendor/model"),
        ["max"]
    );
}

#[tokio::test]
async fn malformed_source_preserves_cache_and_backoff_recovers_independently() {
    let cache = CacheDir::new();
    let server = Server::start().await;
    let loader = server.loader(&cache);
    loader.refresh(false).await.unwrap();
    server.reply(2, StatusCode::OK, r#"{"data":[{"id":42}]}"#);
    loader.refresh(true).await.unwrap();
    assert_eq!(
        loader.state.read().unwrap()[2].error,
        Some(ModelMetadataError::InvalidCatalog)
    );
    assert_eq!(
        loader.snapshot().reasoning_levels_for("model"),
        ["low", "high"]
    );
    loader.state.write().unwrap()[2].retry_at_ms = Some(1);
    server.reply(2, StatusCode::NOT_MODIFIED, "");
    loader.refresh(false).await.unwrap();
    assert_eq!(
        loader.auxiliary_status("openrouter"),
        CatalogStatus::Current
    );
    assert!(loader.state.read().unwrap()[2].retry_at_ms.is_none());
    assert_eq!(
        [
            server.requests(0),
            server.requests(1),
            server.requests(2),
            server.requests(3),
        ],
        [2, 2, 3, 2]
    );
}

#[tokio::test]
async fn cold_304_is_rejected_and_cache_write_failure_keeps_previous_snapshot() {
    let cache = CacheDir::new();
    let server = Server::start().await;
    server.reply(2, StatusCode::NOT_MODIFIED, "");
    let mut loader = server.loader(&cache);
    loader.refresh(false).await.unwrap();
    assert_eq!(
        loader.state.read().unwrap()[2].error,
        Some(ModelMetadataError::InvalidCache)
    );
    let before = loader.snapshot();
    // Existing regular file as a parent makes the cache destination unwritable.
    loader.path = cache.path().join("impossible.json");
    assert_eq!(loader.refresh(true).await, Err(ModelMetadataError::Io));
    assert_eq!(
        loader.snapshot().reasoning_levels_for("model"),
        before.reasoning_levels_for("model")
    );
    assert_ne!(loader.status(), CatalogStatus::Updating);
}

#[test]
fn legacy_cache_migrates_and_corrupt_cache_does_not_block_startup() {
    let cache = CacheDir::new();
    let envelope =
        MetadataCacheEnvelope::new(serde_json::json!({"vendor/model":{"reasoning":true}}), 1)
            .unwrap();
    catalog_io::write_json_if_changed(&cache.path(), &envelope, MAX_CACHE_BYTES).unwrap();
    let loader = ModelMetadataCatalogLoader::open(cache.path()).unwrap();
    assert_eq!(loader.status(), CatalogStatus::Stale);
    assert_eq!(
        loader.snapshot().capabilities_for("model").reasoning,
        Some(true)
    );
    assert!(loader.snapshot().reasoning_levels_for("model").is_empty());
    std::fs::write(cache.path(), "broken").unwrap();
    assert_eq!(
        ModelMetadataCatalogLoader::open(cache.path())
            .unwrap()
            .status(),
        CatalogStatus::Error
    );
}

#[tokio::test]
async fn corrupted_source_hash_does_not_discard_other_sources() {
    let cache = CacheDir::new();
    let server = Server::start().await;
    server.loader(&cache).refresh(false).await.unwrap();
    let mut bundle: Value = catalog_io::read_json(&cache.path(), MAX_CACHE_BYTES)
        .unwrap()
        .unwrap();
    bundle["sources"]["openrouter"]["revision"] = Value::String("tampered".into());
    bundle["mergedPayload"] = serde_json::json!({"malicious":{"reasoning":true}});
    catalog_io::write_json_if_changed(&cache.path(), &bundle, MAX_CACHE_BYTES).unwrap();
    let loader = server.loader(&cache);
    assert_eq!(loader.auxiliary_status("openrouter"), CatalogStatus::Error);
    assert!(loader.snapshot().resolve("vendor/model").is_some());
    assert_eq!(
        loader.snapshot().reasoning_levels_for("vendor/model"),
        ["low"]
    );
    assert!(loader.snapshot().resolve("malicious").is_none());
}

#[test]
fn retry_backoff_is_bounded_and_payload_shapes_are_validated() {
    let mut state = SourceState::new(None, None, 1, 100);
    for expected in [300_000, 1_800_000, 7_200_000, 7_200_000] {
        state.fail(ModelMetadataError::Network, 1);
        assert_eq!(state.deadline(2, 100).at_ms, expected + 1);
    }
    assert!(!valid_payload(
        1,
        &serde_json::json!({"vendor": {"models": {}}})
    ));
    assert!(!valid_payload(2, &serde_json::json!({"data": []})));
    assert!(!valid_payload(3, &serde_json::json!({"sample_spec": {}})));
    assert!(!valid_payload(3, &serde_json::json!({"model": "invalid"})));
}

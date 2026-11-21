use super::*;
use axum::http::{HeaderMap, StatusCode, Uri};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn query(uri: &Uri) -> HashMap<String, String> {
    url::form_urlencoded::parse(uri.query().unwrap_or_default().as_bytes())
        .into_owned()
        .collect()
}

pub(super) async fn source_for_router(
    router: Router,
    wire_api: WireApi,
) -> (ProviderSource, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let prefix = if matches!(wire_api, WireApi::Gemini | WireApi::GeminiInteractions) {
        "v1beta"
    } else {
        "v1"
    };
    (
        ProviderSource {
            id: "synthetic-pagination".into(),
            name: "Synthetic pagination".into(),
            base_url: format!("http://{address}/{prefix}"),
            api_key: "synthetic-key".into(),
            wire_api,
            models: vec!["previous-model".into()],
            proxy_url: None,
        },
        server,
    )
}

#[tokio::test]
async fn messages_catalog_collects_every_page_with_unchanged_authentication() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let observed = requests.clone();
    let router = Router::new().route(
        "/v1/models",
        get(move |uri: Uri, headers: HeaderMap| {
            let observed = observed.clone();
            let query = query(&uri);
            async move {
                observed.lock().unwrap().push((
                    query.clone(),
                    headers
                        .get("x-api-key")
                        .and_then(|value| value.to_str().ok())
                        == Some("synthetic-key"),
                    headers
                        .get("anthropic-version")
                        .and_then(|value| value.to_str().ok())
                        == Some("2023-06-01"),
                ));
                let (id, more) = if query.contains_key("after_id") {
                    ("synthetic-second", false)
                } else {
                    ("synthetic-first", true)
                };
                Json(json!({
                    "data": [{"id": id, "supported_endpoint_types": ["messages"],
                        "inputMicroUsdPerMillion": 10, "outputMicroUsdPerMillion": 20}],
                    "has_more": more,
                    "last_id": "opaque & second?cursor"
                }))
            }
        }),
    );
    let (source, server) = source_for_router(router, WireApi::Messages).await;
    let discovery = discover_source_models_and_protocol_bindings(&source, &[])
        .await
        .unwrap();
    assert_eq!(discovery.models, ["synthetic-first", "synthetic-second"]);
    assert_eq!(discovery.detected_model_prices.len(), 2);
    for model in &discovery.models {
        assert!(discovery.capabilities.iter().any(|row| {
            row.model_id == *model
                && row.upstream_wire_api == WireApi::Messages
                && row.status == CapabilityStatus::Declared
        }));
    }
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].0.is_empty());
    assert_eq!(
        requests[1].0.get("after_id").unwrap(),
        "opaque & second?cursor"
    );
    assert!(requests.iter().all(|(_, key, version)| *key && *version));
    server.abort();
}

#[tokio::test]
async fn gemini_catalog_follows_opaque_tokens_even_when_a_page_has_no_generation_models() {
    for wire_api in [WireApi::Gemini, WireApi::GeminiInteractions] {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = requests.clone();
        let router = Router::new().route(
            "/v1beta/models",
            get(move |uri: Uri| {
                let observed = observed.clone();
                let query = query(&uri);
                async move {
                    observed.lock().unwrap().push(query.clone());
                    if query.contains_key("pageToken") {
                        Json(json!({"models": [{
                            "name": "models/synthetic-generation",
                            "supportedGenerationMethods": ["generateContent"]
                        }]}))
                    } else {
                        Json(json!({"models": [{
                            "name": "models/synthetic-embedding",
                            "supportedGenerationMethods": ["embedContent"]
                        }], "nextPageToken": "opaque & next?token"}))
                    }
                }
            }),
        );
        let (source, server) = source_for_router(router, wire_api).await;
        let discovery = discover_source_models_and_protocol_bindings(&source, &[])
            .await
            .unwrap();
        let expected = if wire_api == WireApi::GeminiInteractions {
            vec!["synthetic-embedding", "synthetic-generation"]
        } else {
            vec!["synthetic-generation"]
        };
        assert_eq!(discovery.models, expected);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].is_empty());
        assert_eq!(requests[1].get("pageToken").unwrap(), "opaque & next?token");
        server.abort();
    }
}

#[tokio::test]
async fn late_page_failures_never_publish_partial_catalogs_and_keep_retry_after() {
    for (status, payload) in [
        (
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error": "synthetic failure"}),
        ),
        (StatusCode::OK, json!({"data": [null], "has_more": false})),
        (
            StatusCode::OK,
            json!({"data": [{"id": "second"}], "has_more": true, "last_id": "next"}),
        ),
        (
            StatusCode::OK,
            json!({"data": [{"id": "second"}], "has_more": true}),
        ),
        (
            StatusCode::OK,
            json!({"data": [{"id": "second"}], "has_more": "yes"}),
        ),
        (
            StatusCode::OK,
            json!({"data": [{"id": "second"}], "has_more": true, "last_id": "x".repeat(8193)}),
        ),
        (
            StatusCode::OK,
            json!({"data": [{"id": "second"}], "nextPageToken": "foreign"}),
        ),
    ] {
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let router = Router::new().route(
            "/v1/models",
            get(move || {
                let observed = observed.clone();
                let payload = payload.clone();
                async move {
                    if observed.fetch_add(1, Ordering::SeqCst) == 0 {
                        (
                            StatusCode::OK,
                            HeaderMap::new(),
                            Json(json!({
                                "data": [{"id": "first", "inputMicroUsdPerMillion": 1,
                                    "outputMicroUsdPerMillion": 2}],
                                "has_more": true, "last_id": "next"
                            })),
                        )
                    } else {
                        let mut headers = HeaderMap::new();
                        if status == StatusCode::TOO_MANY_REQUESTS {
                            headers.insert("retry-after", "3".parse().unwrap());
                        }
                        (status, headers, Json(payload))
                    }
                }
            }),
        );
        let (source, server) = source_for_router(router, WireApi::Messages).await;
        let read = read_source_models(&source, &[], &SourceProtocolConfig::default()).await;
        assert!(read.read_value.is_err());
        if status == StatusCode::TOO_MANY_REQUESTS {
            assert!(matches!(
                read.read_value,
                Err(crate::Error::UpstreamStatus(429))
            ));
            assert_eq!(read.retry_after_ms, Some(3000));
        }
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        server.abort();
    }
}

#[tokio::test]
async fn catalog_body_limit_applies_to_all_pages_together() {
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let router = Router::new().route(
        "/v1/models",
        get(move || {
            let observed = observed.clone();
            async move {
                let page = observed.fetch_add(1, Ordering::SeqCst);
                Json(json!({"data": [{"id": format!("synthetic-{page}")}],
                    "has_more": page == 0, "last_id": "next",
                    "padding": "x".repeat(crate::transport::MAX_MODEL_CATALOG_BODY_BYTES / 2 + 1)}))
            }
        }),
    );
    let (source, server) = source_for_router(router, WireApi::Messages).await;
    let result = discover_source_models_and_protocol_bindings(&source, &[]).await;
    assert!(matches!(result, Err(crate::Error::UpstreamBodyTooLarge)));
    assert_eq!(requests.load(Ordering::SeqCst), 2);
    server.abort();
}

#[tokio::test]
async fn distinct_cursors_cannot_create_an_unbounded_catalog_read() {
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let router = Router::new().route(
        "/v1/models",
        get(move || {
            let observed = observed.clone();
            async move {
                let page = observed.fetch_add(1, Ordering::SeqCst);
                Json(json!({"data": [{"id": format!("synthetic-{page}")}],
                    "has_more": true, "last_id": format!("cursor-{page}")}))
            }
        }),
    );
    let (source, server) = source_for_router(router, WireApi::Messages).await;
    let result = discover_source_models_and_protocol_bindings(&source, &[]).await;
    assert!(matches!(
        result,
        Err(crate::Error::InvalidUpstreamResponse(
            "upstream model pagination exceeds its page limit"
        ))
    ));
    assert_eq!(requests.load(Ordering::SeqCst), 128);
    server.abort();
}

#[tokio::test]
async fn catalog_deadline_is_shared_by_all_pages() {
    let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
    let router = Router::new().route(
        "/v1/models",
        get(move |uri: Uri| {
            let started = started.clone();
            let query = query(&uri);
            async move {
                let second = query.contains_key("after_id");
                started.send(second).unwrap();
                tokio::time::sleep(std::time::Duration::from_secs(if second { 60 } else { 20 }))
                    .await;
                Json(json!({"data": [{"id": "synthetic"}], "has_more": !second, "last_id": "next"}))
            }
        }),
    );
    let (source, server) = source_for_router(router, WireApi::Messages).await;
    let read =
        tokio::spawn(
            async move { discover_source_models_and_protocol_bindings(&source, &[]).await },
        );
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), starts.recv())
            .await
            .unwrap(),
        Some(false)
    );
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(21)).await;
    tokio::time::resume();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), starts.recv())
            .await
            .unwrap(),
        Some(true)
    );
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    tokio::time::resume();
    assert!(matches!(
        read.await.unwrap(),
        Err(crate::Error::ManagementHttpUnavailable)
    ));
    server.abort();
}

#[tokio::test]
async fn openai_catalog_does_not_guess_another_protocols_pagination_contract() {
    for wire_api in [WireApi::Responses, WireApi::ChatCompletions] {
        for cursor in [
            json!({"has_more": true, "last_id": "next"}),
            json!({"nextPageToken": "next"}),
        ] {
            let requests = Arc::new(AtomicUsize::new(0));
            let observed = requests.clone();
            let mut payload = cursor;
            payload["data"] = json!([{"id": "synthetic"}]);
            let router = Router::new().route(
                "/v1/models",
                get(move || {
                    observed.fetch_add(1, Ordering::SeqCst);
                    let payload = payload.clone();
                    async move { Json(payload) }
                }),
            );
            let (source, server) = source_for_router(router, wire_api).await;
            let result = discover_source_models_and_protocol_bindings(&source, &[]).await;
            assert!(matches!(
                result,
                Err(crate::Error::InvalidUpstreamResponse(
                    "upstream model pagination is unsupported"
                ))
            ));
            assert_eq!(requests.load(Ordering::SeqCst), 1);
            server.abort();
        }
    }
}

#[tokio::test]
async fn protobuf_empty_model_pages_preserve_pagination_and_reject_error_objects() {
    for wire_api in [WireApi::Gemini, WireApi::GeminiInteractions] {
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let router = Router::new().route(
            "/v1beta/models",
            get(move || {
                let observed = observed.clone();
                async move {
                    if observed.fetch_add(1, Ordering::SeqCst) == 0 {
                        Json(json!({"nextPageToken": "next"}))
                    } else {
                        Json(json!({}))
                    }
                }
            }),
        );
        let (source, server) = source_for_router(router, wire_api).await;
        let discovery = discover_source_models_and_protocol_bindings(&source, &[])
            .await
            .unwrap();
        assert!(discovery.models.is_empty());
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert!(
            parse_upstream_models(
                if wire_api == WireApi::Gemini {
                    UpstreamProtocol::GeminiGenerateContent
                } else {
                    UpstreamProtocol::GeminiInteractions
                },
                &json!({"error": "synthetic failure"}),
            )
            .is_none()
        );
        server.abort();
    }
}

#[tokio::test]
async fn overlapping_pages_preserve_inventory_order_without_duplicate_binding_models() {
    let router = Router::new().route(
        "/v1/models",
        get(|uri: Uri| async move {
            let query = query(&uri);
            if query.contains_key("after_id") {
                Json(json!({"data": [{"id": "second"}, {"id": "third"}], "has_more": false}))
            } else {
                Json(json!({"data": [{"id": "first"}, {"id": "second"}], "has_more": true, "last_id": "next"}))
            }
        }),
    );
    let (mut source, server) = source_for_router(router, WireApi::Messages).await;
    source.models = vec!["first".into(), "second".into(), "third".into()];
    let binding = SourceProtocolConfig::default()
        .resolve(
            &source.base_url,
            &["first".into(), "second".into()],
            &[],
            WireApi::Messages,
        )
        .unwrap()
        .into_iter()
        .find(|binding| binding.wire_api == WireApi::Messages)
        .unwrap();
    let discovery = discover_source_models_and_protocol_bindings(&source, &[binding])
        .await
        .unwrap();
    assert_eq!(discovery.models, ["first", "second", "third"]);
    assert_eq!(discovery.protocol_bindings.len(), 1);
    assert_eq!(
        discovery.protocol_bindings[0].model_ids,
        ["first", "second"]
    );
    server.abort();
}

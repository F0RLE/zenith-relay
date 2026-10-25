use super::*;
use crate::scheduler::rotation::SharedRequestBudget;
use crate::{
    GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeLocalKey, RuntimeSource, WireApi,
};
use axum::http::HeaderValue;
use std::{collections::HashSet, sync::Mutex};

type Events = Arc<Mutex<Vec<UsageEvent>>>;

async fn image_state(chunks: Vec<&'static str>) -> (ImageStream, Events, SharedRequestBudget) {
    let events: Events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let runtime = Arc::new(
        GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: "source".into(),
                name: "Synthetic".into(),
                base_url: "https://example.invalid/v1".into(),
                api_key: "synthetic".into(),
                wire_api: WireApi::Responses,
                models: vec!["gpt-image-test".into()],
                proxy_url: None,
            })],
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "key".into(),
                secret: "synthetic".into(),
            })],
            GatewayRuntimeOptions::default(),
            Arc::new(move |event| observed.lock().unwrap().push(event)),
        )
        .unwrap(),
    );
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer synthetic")))
        .unwrap();
    let budget = SharedRequestBudget::for_incoming_request(3);
    let (_, lease) = runtime
        .select_and_reserve_image_with_budget(
            &key,
            "gpt-image-test",
            &[WireApi::Responses],
            &HashSet::new(),
            now_ms(),
            &budget,
        )
        .await
        .unwrap();
    lease.begin_rotation_http_dispatch().unwrap();
    let mut event = crate::gateway::test_support::test_usage_event();
    event.resolved_model = Some("gpt-image-test".into());
    event.image_usage = Some(Default::default());
    let state = ImageStream {
        upstream: Box::pin(futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|chunk| Ok(Bytes::from_static(chunk.as_bytes()))),
        )),
        pending: Vec::new(),
        completion: ImageCompletion {
            runtime,
            lease: Arc::new(lease),
            event: Some(event),
            started: Instant::now(),
        },
        terminal_kind: "image_generation.completed".into(),
        expected_images: 1,
        completed_images: 0,
        finished: false,
    };
    (state, events, budget)
}

const PARTIAL: &str = "data: {\"type\":\"image_generation.partial_image\",\"b64_json\":\"cHJldmlldw==\",\"partial_image_index\":0,\"usage\":{\"input_tokens\":5}}\n\n";
const COMPLETED: &str = "data: {\"type\":\"image_generation.completed\",\"b64_json\":\"aW1hZ2U=\",\"usage\":{\"input_tokens\":5,\"output_tokens\":7,\"total_tokens\":12,\"input_tokens_details\":{\"text_tokens\":2,\"image_tokens\":3}}}\n\n";

#[tokio::test]
async fn native_image_partials_are_delivered_before_completion_and_counted_once() {
    let (mut state, events, _) = image_state(vec![PARTIAL, COMPLETED]).await;
    assert_eq!(state.next().await.unwrap(), PARTIAL);
    assert!(events.lock().unwrap().is_empty());
    assert_eq!(state.next().await.unwrap(), COMPLETED);
    assert!(state.next().await.is_none());
    drop(state);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].total_tokens, Some(12));
    assert_eq!(
        events[0].image_usage.as_ref().unwrap().input_image_tokens,
        Some(3)
    );
    assert_eq!(events[0].image_usage.as_ref().unwrap().generated_images, 1);
}

#[tokio::test]
async fn native_image_client_drop_keeps_usage_and_forbids_replay() {
    let (mut state, events, budget) = image_state(vec![PARTIAL]).await;
    state.next().await.unwrap();
    drop(state);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].input_tokens, Some(5));
    assert_eq!(
        events[0].error_category.as_deref(),
        Some(error_codes::CLIENT_CANCELLED)
    );
    assert!(!budget.can_dispatch());
}

#[tokio::test]
async fn native_image_unpolled_body_drop_settles_its_lease() {
    let (state, events, budget) = image_state(vec![COMPLETED]).await;
    drop(state);
    assert_eq!(events.lock().unwrap().len(), 1);
    assert!(!budget.can_dispatch());
}

#[tokio::test]
async fn native_image_truncated_invalid_and_error_streams_never_succeed_or_retry() {
    for chunks in [vec![PARTIAL], vec!["data: invalid\n\n"], vec![
        "data: {\"type\":\"error\",\"error\":{\"code\":\"synthetic_error\"},\"usage\":{\"output_tokens\":4}}\n\n"
    ], vec!["data: {\"type\":\"image_generation.completed\"}\n\n"]] {
        let (mut state, events, budget) = image_state(chunks).await;
        while state.next().await.is_some() {}
        drop(state);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].success);
        assert!(!budget.can_dispatch());
        if events[0].error_category.as_deref() == Some(error_codes::STREAM_ERROR) {
            assert_eq!(events[0].output_tokens, Some(4));
        }
    }
}

#[tokio::test]
async fn native_image_fragmented_frames_and_keepalives_are_preserved() {
    let (mut state, events, _) = image_state(vec![
        ": keepalive\n\n",
        "data: {\"type\":\"image_generation.",
        "completed\",\"b64_json\":\"aW1hZ2U=\"}\r\n\r\n",
    ])
    .await;
    assert_eq!(state.next().await.unwrap(), ": keepalive\n\n");
    let completion = state.next().await.unwrap();
    assert!(String::from_utf8_lossy(&completion).contains("image_generation.completed"));
    assert!(state.next().await.is_none());
    assert!(events.lock().unwrap()[0].success);
}

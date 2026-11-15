use super::*;
use crate::scheduler::rotation::SharedRequestBudget;
use crate::{
    GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeLocalKey, RuntimeSource, WireApi,
};
use axum::http::HeaderValue;
use std::{collections::HashSet, sync::Mutex};

type Events = Arc<Mutex<Vec<UsageEvent>>>;

async fn image_state(chunks: Vec<&'static str>) -> (ImageStream, Events, SharedRequestBudget) {
    image_state_with_debug(chunks, false).await
}

async fn image_state_with_debug(
    chunks: Vec<&'static str>,
    debug: bool,
) -> (ImageStream, Events, SharedRequestBudget) {
    image_state_with_gate(chunks, if debug { || true } else { || false }).await
}

async fn image_state_with_gate(
    chunks: Vec<&'static str>,
    diagnostics_enabled: fn() -> bool,
) -> (ImageStream, Events, SharedRequestBudget) {
    let debug = diagnostics_enabled();
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
            GatewayRuntimeOptions {
                diagnostics_enabled,
                ..Default::default()
            },
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
    if debug {
        event.routing = Some(
            serde_json::from_value(serde_json::json!({
                "reason":"only_eligible", "eligibleCandidates":1, "quotaRemainingBasisPoints":null,
                "inFlightBefore":0, "dispatchesBefore":0,
                "images":{"requested":{"size":"3840x2160"},"sent":{"size":"3840x2160"}}
            }))
            .unwrap(),
        );
    }
    let state = ImageStream {
        upstream: Box::pin(futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|chunk| Ok(Bytes::from_static(chunk.as_bytes()))),
        )),
        pending: Vec::new(),
        incoming: Bytes::new(),
        event_limit: MAX_IMAGE_EVENT_BYTES,
        completion: ImageCompletion {
            runtime,
            lease: Arc::new(lease),
            event: Some(event),
            started: Instant::now(),
        },
        terminal_kind: "image_generation.completed".into(),
        expected_images: 1,
        completed_images: 0,
        normalize_codex_edits: false,
        usage: ImageUsageState::default(),
        finished: false,
    };
    (state, events, budget)
}

const PARTIAL: &str = "data: {\"type\":\"image_generation.partial_image\",\"b64_json\":\"cHJldmlldw==\",\"partial_image_index\":0,\"usage\":{\"input_tokens\":5}}\n\n";
const COMPLETED: &str = "data: {\"type\":\"image_generation.completed\",\"b64_json\":\"aW1hZ2U=\",\"usage\":{\"input_tokens\":5,\"output_tokens\":7,\"total_tokens\":12,\"input_tokens_details\":{\"text_tokens\":2,\"image_tokens\":3}}}\n\n";

#[tokio::test]
async fn image_diagnostics_ignore_previews_and_keep_completed_outputs_after_cancel() {
    let (mut state, events, _) = image_state_with_debug(vec![PARTIAL, COMPLETED], true).await;
    state.expected_images = 2;
    assert_eq!(state.next().await.unwrap(), PARTIAL);
    assert!(
        state
            .completion
            .event
            .as_ref()
            .unwrap()
            .routing
            .as_ref()
            .unwrap()
            .images
            .as_ref()
            .unwrap()
            .outputs
            .is_empty()
    );
    assert_eq!(state.next().await.unwrap(), COMPLETED);
    drop(state);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some(error_codes::CLIENT_CANCELLED)
    );
    let diagnostics = events[0].routing.as_ref().unwrap().images.as_ref().unwrap();
    assert_eq!(diagnostics.reported.len(), 1);
    assert_eq!(diagnostics.outputs.len(), 1);
    assert_eq!(events[0].image_usage.as_ref().unwrap().generated_images, 1);
}

#[tokio::test]
async fn image_diagnostics_stay_absent_or_are_removed_when_disabled_midstream() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static ENABLED: AtomicBool = AtomicBool::new(true);
    let (mut state, events, _) = image_state(vec![PARTIAL, COMPLETED]).await;
    while state.next().await.is_some() {}
    assert!(
        events.lock().unwrap()[0]
            .routing
            .as_ref()
            .and_then(|routing| routing.images.as_ref())
            .is_none()
    );
    for disable_before_output in [false, true] {
        ENABLED.store(true, Ordering::Relaxed);
        let (mut state, events, _) =
            image_state_with_gate(vec![COMPLETED], || ENABLED.load(Ordering::Relaxed)).await;
        state.expected_images = 2;
        if disable_before_output {
            ENABLED.store(false, Ordering::Relaxed);
        }
        state.next().await.unwrap();
        ENABLED.store(false, Ordering::Relaxed);
        drop(state);
        assert!(
            events.lock().unwrap()[0]
                .routing
                .as_ref()
                .unwrap()
                .images
                .is_none()
        );
    }
}

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
    for chunks in [
        vec![PARTIAL],
        vec!["data: invalid\n\n"],
        vec![
            "data: {\"type\":\"error\",\"error\":{\"code\":\"synthetic_error\"},\"usage\":{\"output_tokens\":4}}\n\n",
        ],
        vec!["data: {\"type\":\"image_generation.completed\"}\n\n"],
    ] {
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

#[tokio::test]
async fn codex_edit_stream_normalizes_event_names_without_changing_image_or_extensions() {
    let completed = "id: synthetic\nevent: image_generation.completed\nretry: 1000\ndata: {\"type\":\"image_generation.completed\",\"b64_json\":\"aW1hZ2U=\",\"future\":\"image_generation.completed\",\"usage\":{\"input_tokens\":5,\"output_tokens\":7}}\n\n";
    let (mut state, events, _) = image_state(vec![PARTIAL, completed]).await;
    state.terminal_kind = "image_edit.completed".into();
    state.normalize_codex_edits = true;
    let partial = state.next().await.unwrap();
    let payload: serde_json::Value =
        serde_json::from_slice(&crate::protocol::sse::event_data(&partial)).unwrap();
    assert_eq!(payload["type"], "image_edit.partial_image");
    assert!(events.lock().unwrap().is_empty());
    let completion = state.next().await.unwrap();
    let payload: serde_json::Value =
        serde_json::from_slice(&crate::protocol::sse::event_data(&completion)).unwrap();
    assert_eq!(payload["type"], "image_edit.completed");
    assert_eq!(payload["b64_json"], "aW1hZ2U=");
    assert_eq!(payload["future"], "image_generation.completed");
    let text = String::from_utf8_lossy(&completion);
    assert!(text.contains("event: image_edit.completed\n"));
    assert!(text.contains("id: synthetic\n"));
    assert!(text.contains("retry: 1000\n"));
    assert!(state.next().await.is_none());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].total_tokens, Some(12));
    assert_eq!(events[0].image_usage.as_ref().unwrap().generated_images, 1);
}

#[tokio::test]
async fn native_edit_event_names_remain_exact_and_api_sources_are_not_rewritten() {
    const EDIT: &str = "event: image_edit.completed\ndata: {\"type\":\"image_edit.completed\",\"b64_json\":\"aW1hZ2U=\"}\n\n";
    for normalize in [false, true] {
        let (mut state, events, _) = image_state(vec![EDIT]).await;
        state.terminal_kind = "image_edit.completed".into();
        state.normalize_codex_edits = normalize;
        assert_eq!(state.next().await.unwrap(), EDIT);
        assert!(state.next().await.is_none());
        assert!(events.lock().unwrap()[0].success);
    }
    let (mut state, events, _) = image_state(vec![COMPLETED]).await;
    state.terminal_kind = "image_edit.completed".into();
    assert_eq!(state.next().await.unwrap(), COMPLETED);
    while state.next().await.is_some() {}
    assert!(!events.lock().unwrap()[0].success);
}

#[tokio::test]
async fn image_event_limit_is_independent_of_network_chunk_boundaries() {
    let (mut state, events, _) = image_state(Vec::new()).await;
    state.expected_images = 2;
    state.event_limit = COMPLETED.len();
    state.upstream = Box::pin(futures_util::stream::iter([Ok(Bytes::from(format!(
        "{PARTIAL}{COMPLETED}{COMPLETED}"
    )))]));
    assert_eq!(state.next().await.unwrap(), PARTIAL);
    assert_eq!(state.next().await.unwrap(), COMPLETED);
    assert!(events.lock().unwrap().is_empty());
    assert_eq!(state.next().await.unwrap(), COMPLETED);
    assert!(state.next().await.is_none());
    assert!(state.pending.len() <= state.event_limit);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].image_usage.as_ref().unwrap().generated_images, 2);
}

#[tokio::test]
async fn oversized_single_image_event_fails_without_replaying_or_losing_usage() {
    let (mut state, events, budget) = image_state(vec![PARTIAL, COMPLETED]).await;
    state.event_limit = PARTIAL.len();
    assert_eq!(state.next().await.unwrap(), PARTIAL);
    let error = state.next().await.unwrap();
    assert!(String::from_utf8_lossy(&error).contains(error_codes::STREAM_EVENT_TOO_LARGE));
    assert!(state.next().await.is_none());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].input_tokens, Some(5));
    assert!(!events[0].success);
    assert!(!budget.can_dispatch());
}

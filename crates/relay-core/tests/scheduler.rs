use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE};
use axum::http::{HeaderMap, Response, StatusCode, Uri};
use axum::routing::{get, post};
use axum::Router;
use futures_util::stream;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use zenith_relay_core::gateway;
use zenith_relay_core::{
    DefaultServiceTier, GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey, ProviderSource,
    RuntimeLocalKey, RuntimeSource, SourceProtocolBinding, UsageEvent, WireApi,
};

const LOCAL_KEY: &str = "p2-local-key";
const MODEL: &str = "gpt-p2";

#[derive(Clone, Debug)]
struct ObservedRequest {
    path: String,
    authorization: Option<String>,
    x_api_key: Option<String>,
    anthropic_version: Option<String>,
    anthropic_beta: Option<String>,
    claude_code_session_id: Option<String>,
    body: Value,
}

#[derive(Clone)]
enum Reply {
    Json {
        status: StatusCode,
        body: Value,
        cache_control: &'static str,
        retry_after: Option<&'static str>,
    },
    Stream {
        chunks: Vec<StreamChunk>,
        cache_control: &'static str,
    },
}

#[derive(Clone)]
enum StreamChunk {
    Data(&'static str),
    Error,
}

#[derive(Clone)]
struct UpstreamState {
    key: String,
    replies: Arc<Mutex<VecDeque<Reply>>>,
    requests: Arc<Mutex<Vec<ObservedRequest>>>,
}

struct TestServer {
    base_url: String,
    task: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[tokio::test]
async fn models_union_respects_each_local_key_scope_without_upstream_calls() {
    let (source_a, state_a) = spawn_upstream("source-a-key", Vec::new()).await;
    let (source_b, state_b) = spawn_upstream("source-b-key", Vec::new()).await;
    let (gateway, _) = spawn_gateway(
        vec![
            source(
                "source-a",
                &source_a,
                "source-a-key",
                &["alpha", "shared"],
                0,
            ),
            source(
                "source-b",
                &source_b,
                "source-b-key",
                &["beta", "shared"],
                0,
            ),
        ],
        vec![
            local_key("all", LOCAL_KEY, None),
            local_key("scoped", "scoped-key", Some(vec!["source-a"])),
            local_key("empty", "empty-key", Some(Vec::new())),
        ],
        3,
    )
    .await;

    assert_eq!(
        models(&gateway, LOCAL_KEY).await,
        ["alpha", "shared", "beta"]
    );
    assert_eq!(models(&gateway, "scoped-key").await, ["alpha", "shared"]);
    assert!(models(&gateway, "empty-key").await.is_empty());
    assert!(state_a.requests.lock().unwrap().is_empty());
    assert!(state_b.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn public_models_preserve_the_first_source_response_order() {
    let (upstream, _) = spawn_upstream("source-key", Vec::new()).await;
    let (gateway, _) = spawn_gateway(
        vec![source(
            "source",
            &upstream,
            "source-key",
            &[
                "private-second",
                "vendor/grok-4.5",
                "vendor/glm-4.7",
                "vendor/gemini-3.6-flash-low",
                "vendor/claude-haiku-4-5",
                "gpt-image-2",
                "gpt-5.4-mini",
                "gpt-5.6-sol",
                "vendor/glm-5.2",
                "private-first",
            ],
            0,
        )],
        vec![local_key("all", LOCAL_KEY, None)],
        3,
    )
    .await;

    assert_eq!(
        models(&gateway, LOCAL_KEY).await,
        [
            "private-second",
            "vendor/grok-4.5",
            "vendor/glm-4.7",
            "vendor/gemini-3.6-flash-low",
            "vendor/claude-haiku-4-5",
            "gpt-image-2",
            "gpt-5.4-mini",
            "gpt-5.6-sol",
            "vendor/glm-5.2",
            "private-first",
        ]
    );
}

#[tokio::test]
async fn fast_for_all_forces_priority_over_client_service_tier() {
    let (upstream, state) = spawn_upstream("source-key", Vec::new()).await;
    let (gateway, _) = spawn_gateway_with_options(
        vec![source("source", &upstream, "source-key", &[MODEL], 0)],
        vec![local_key("key", LOCAL_KEY, None)],
        GatewayRuntimeOptions {
            max_retry_candidates: 3,
            default_service_tier: DefaultServiceTier::Fast,
            ..GatewayRuntimeOptions::default()
        },
    )
    .await;

    for tier in [
        None,
        Some("fast"),
        Some("standard"),
        Some("flex"),
        Some("default"),
        Some("priority"),
    ] {
        let mut body = json!({"model": MODEL, "input": "hello"});
        if let Some(tier) = tier {
            body["service_tier"] = Value::String(tier.to_string());
        }
        let response = reqwest::Client::new()
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let requests = state.requests.lock().unwrap();
    let tiers = requests
        .iter()
        .map(|request| request.body["service_tier"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        tiers,
        [
            Some("priority"),
            Some("priority"),
            Some("priority"),
            Some("priority"),
            Some("priority"),
            Some("priority"),
        ]
    );
}

#[tokio::test]
async fn five_xx_falls_back_with_isolated_credentials_and_cools_the_failed_source() {
    let (source_a, state_a) = spawn_upstream(
        "source-a-key",
        vec![status_reply(StatusCode::SERVICE_UNAVAILABLE, "loser", None)],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "source-b-key",
        vec![
            response_reply("resp-b-1", "winner"),
            response_reply("resp-b-2", "winner"),
        ],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("source-a", &source_a, "source-a-key", &[MODEL], 10),
            source("source-b", &source_b, "source-b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let first = request(&gateway, false).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()[CACHE_CONTROL], "winner");
    assert_eq!(first.json::<Value>().await.unwrap()["id"], "resp-b-1");
    assert_eq!(
        request(&gateway, false)
            .await
            .json::<Value>()
            .await
            .unwrap()["id"],
        "resp-b-2"
    );

    let a = state_a.requests.lock().unwrap();
    let b = state_b.requests.lock().unwrap();
    assert_eq!(a.len(), 1, "5xx source should remain in cooldown");
    assert_eq!(b.len(), 2);
    assert_eq!(a[0].authorization.as_deref(), Some("Bearer source-a-key"));
    assert_eq!(b[0].authorization.as_deref(), Some("Bearer source-b-key"));
    assert!(!a[0].authorization.as_deref().unwrap().contains(LOCAL_KEY));
    drop(a);
    drop(b);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        (
            events[0].attempt,
            events[0].source_id.as_str(),
            events[0].success
        ),
        (1, "source-a", false)
    );
    assert_eq!(
        (
            events[1].attempt,
            events[1].source_id.as_str(),
            events[1].success
        ),
        (2, "source-b", true)
    );
    assert_eq!(
        (
            events[2].attempt,
            events[2].source_id.as_str(),
            events[2].success
        ),
        (1, "source-b", true)
    );
}

#[tokio::test]
async fn rate_limit_retry_after_cools_source_before_the_next_request() {
    let (source_a, state_a) = spawn_upstream(
        "source-a-key",
        vec![status_reply(
            StatusCode::TOO_MANY_REQUESTS,
            "limited",
            Some("60"),
        )],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "source-b-key",
        vec![
            response_reply("resp-b-1", "ready"),
            response_reply("resp-b-2", "ready"),
        ],
    )
    .await;
    let (gateway, _) = spawn_gateway(
        vec![
            source("source-a", &source_a, "source-a-key", &[MODEL], 10),
            source("source-b", &source_b, "source-b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert_eq!(state_b.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn all_cooled_sources_keep_model_visible_and_return_local_retry_after() {
    let (source_a, state_a) = spawn_upstream(
        "source-a-key",
        vec![status_reply(StatusCode::TOO_MANY_REQUESTS, "a", None)],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "source-b-key",
        vec![status_reply(StatusCode::TOO_MANY_REQUESTS, "b", None)],
    )
    .await;
    let (gateway, _) = spawn_gateway(
        vec![
            source("source-a", &source_a, "source-a-key", &[MODEL], 10),
            source("source-b", &source_b, "source-b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let first = request(&gateway, false).await;
    assert_eq!(first.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        first.headers()["retry-after"]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 1
    );
    assert_eq!(models(&gateway, LOCAL_KEY).await, [MODEL]);
    let before = (
        state_a.requests.lock().unwrap().len(),
        state_b.requests.lock().unwrap().len(),
    );

    let second = request(&gateway, false).await;
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        second.headers()["retry-after"]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 1
    );
    let body: Value = second.json().await.unwrap();
    assert_eq!(body["error"]["code"], "all_sources_cooling_down");
    assert_eq!(
        before,
        (
            state_a.requests.lock().unwrap().len(),
            state_b.requests.lock().unwrap().len(),
        )
    );
}

#[tokio::test]
async fn bad_request_is_terminal_and_does_not_call_the_fallback_source() {
    let (source_a, state_a) = spawn_upstream(
        "source-a-key",
        vec![status_reply(StatusCode::BAD_REQUEST, "bad", None)],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "source-b-key",
        vec![response_reply("must-not-run", "fallback")],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("source-a", &source_a, "source-a-key", &[MODEL], 10),
            source("source-b", &source_b, "source-b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    assert_eq!(
        request(&gateway, false).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert!(state_b.requests.lock().unwrap().is_empty());
    assert_eq!(events.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn overloaded_bad_request_falls_back_and_cools_only_the_model() {
    let (source_a, state_a) = spawn_upstream(
        "source-a-key",
        vec![Reply::Json {
            status: StatusCode::BAD_REQUEST,
            body: json!({
                "error": {
                    "type": "invalid_request_error",
                    "code": "server_is_overloaded",
                    "message": "Please retry later."
                }
            }),
            cache_control: "overloaded",
            retry_after: None,
        }],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "source-b-key",
        vec![response_reply("fallback-response", "fallback")],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("source-a", &source_a, "source-a-key", &[MODEL], 10),
            source("source-b", &source_b, "source-b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "fallback-response"
    );
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert_eq!(state_b.requests.lock().unwrap().len(), 1);

    let events = events.lock().unwrap();
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_overloaded")
    );
    assert_eq!(events[0].cooldown_scope.as_deref(), Some(MODEL));
}

#[tokio::test]
async fn retry_budget_counts_execution_attempts_and_stops_before_third_source() {
    let (source_a, state_a) = spawn_upstream(
        "a-key",
        vec![status_reply(StatusCode::SERVICE_UNAVAILABLE, "a", None)],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "b-key",
        vec![status_reply(StatusCode::SERVICE_UNAVAILABLE, "b", None)],
    )
    .await;
    let (source_c, state_c) = spawn_upstream(
        "c-key",
        vec![status_reply(StatusCode::SERVICE_UNAVAILABLE, "c", None)],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 3),
            source("b", &source_b, "b-key", &[MODEL], 2),
            source("c", &source_c, "c-key", &[MODEL], 1),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        2,
    )
    .await;

    assert_eq!(
        request(&gateway, false).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert_eq!(state_b.requests.lock().unwrap().len(), 1);
    assert!(state_c.requests.lock().unwrap().is_empty());
    assert_eq!(events.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn unmapped_candidate_is_tried_without_spending_the_execution_budget() {
    let (chat_server, chat_state) = spawn_upstream("chat-key", Vec::new()).await;
    let (responses_server, responses_state) = spawn_upstream(
        "responses-key",
        vec![response_reply("responses-wins", "winner")],
    )
    .await;
    let mut chat = source("chat", &chat_server, "chat-key", &[MODEL], 10);
    chat.source.wire_api = WireApi::ChatCompletions;
    let (gateway, events) = spawn_gateway(
        vec![
            chat,
            source("responses", &responses_server, "responses-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        1,
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "hello",
            "previous_response_id": "resp_previous"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "responses-wins"
    );
    assert!(chat_state.requests.lock().unwrap().is_empty());
    assert_eq!(responses_state.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].attempt, 1);
    assert_eq!(events[0].source_id, "responses");
}

#[tokio::test]
async fn stream_bytes_commit_the_response_before_the_first_complete_event() {
    let (source_a, state_a) = spawn_upstream(
        "a-key",
        vec![Reply::Stream {
            chunks: vec![
                StreamChunk::Data("data: {\"type\":\"response.created\""),
                StreamChunk::Error,
            ],
            cache_control: "loser",
        }],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "b-key",
        vec![Reply::Stream {
            chunks: vec![
                StreamChunk::Data("data: {\"type\":\"response."),
                StreamChunk::Data("created\"}\n\n"),
                StreamChunk::Data("data: [DONE]\n\n"),
            ],
            cache_control: "winner",
        }],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 10),
            source("b", &source_b, "b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CACHE_CONTROL], "loser");
    let body = response.text().await.unwrap();
    assert_eq!(body.matches("response.created").count(), 1);
    assert!(body.contains("response.failed"));
    assert!(!body.contains("[DONE]"));
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert!(state_b.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(events[0].error_category.as_deref(), Some("upstream_stream"));
}

#[tokio::test]
async fn streaming_usage_limit_keeps_the_provider_reset_before_fallback() {
    let (source_a, _) = spawn_upstream(
        "a-key",
        vec![Reply::Stream {
            chunks: vec![StreamChunk::Data(
                "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"type\":\"usage_limit_reached\",\"resets_in_seconds\":120}}}\n\n",
            )],
            cache_control: "limited",
        }],
    )
    .await;
    let (source_b, _) = spawn_upstream(
        "b-key",
        vec![Reply::Stream {
            chunks: vec![
                StreamChunk::Data("data: {\"type\":\"response.created\"}\n\n"),
                StreamChunk::Data("data: [DONE]\n\n"),
            ],
            cache_control: "winner",
        }],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 10),
            source("b", &source_b, "b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CACHE_CONTROL], "winner");
    let _ = response.text().await.unwrap();

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let events = events.lock().unwrap();
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_quota_exhausted")
    );
    assert_eq!(events[0].cooldown_scope.as_deref(), Some("*"));
    assert!(events[0]
        .retry_at_ms
        .is_some_and(|retry_at| retry_at > now_ms + 100_000));
    assert!(events[1].success);
}

#[tokio::test]
async fn streaming_plan_entitlement_failure_falls_back_without_blocking_the_account() {
    let (source_a, _) = spawn_upstream(
        "a-key",
        vec![Reply::Stream {
            chunks: vec![StreamChunk::Data(
                "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"usage_not_included\"}}}\n\n",
            )],
            cache_control: "limited",
        }],
    )
    .await;
    let (source_b, _) = spawn_upstream(
        "b-key",
        vec![Reply::Stream {
            chunks: vec![StreamChunk::Data("data: [DONE]\n\n")],
            cache_control: "winner",
        }],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 10),
            source("b", &source_b, "b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CACHE_CONTROL], "winner");
    let _ = response.text().await.unwrap();

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].http_status, StatusCode::FORBIDDEN.as_u16());
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_usage_not_included")
    );
    assert_eq!(events[0].cooldown_scope.as_deref(), Some("*"));
    assert!(events[1].success);
}

#[tokio::test]
async fn streaming_invalid_prompt_is_terminal_and_does_not_spend_the_fallback() {
    let (source_a, state_a) = spawn_upstream(
        "a-key",
        vec![Reply::Stream {
            chunks: vec![StreamChunk::Data(
                "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"invalid_prompt\",\"message\":\"Invalid prompt\"}}}\n\n",
            )],
            cache_control: "rejected",
        }],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "b-key",
        vec![Reply::Stream {
            chunks: vec![StreamChunk::Data("data: [DONE]\n\n")],
            cache_control: "must-not-run",
        }],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 10),
            source("b", &source_b, "b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.text().await.unwrap();
    assert!(body.contains("invalid_request_error"), "body={body}");
    assert!(body.contains("invalid_request"), "body={body}");
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert!(state_b.requests.lock().unwrap().is_empty());

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_invalid_request")
    );
    assert!(events[0].cooldown_scope.is_none());
}

#[tokio::test]
async fn stream_does_not_fallback_after_a_native_prelude_reaches_the_client() {
    let (source_a, state_a) = spawn_upstream(
        "a-key",
        vec![Reply::Stream {
            chunks: vec![
                StreamChunk::Data("data: {\"type\":\"response.created\"}\n\n"),
                StreamChunk::Error,
            ],
            cache_control: "first",
        }],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "b-key",
        vec![Reply::Stream {
            chunks: vec![StreamChunk::Data("data: [DONE]\n\n")],
            cache_control: "winner",
        }],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 10),
            source("b", &source_b, "b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let response = request(&gateway, true).await;
    assert_eq!(response.headers()[CACHE_CONTROL], "first");
    let body = response.text().await.unwrap();
    assert!(body.contains("response.created"));
    assert!(body.contains("response.failed"));
    assert!(!body.contains("[DONE]"));
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert!(state_b.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(events[0].error_category.as_deref(), Some("upstream_stream"));
}

#[tokio::test]
async fn invalid_sse_after_a_native_prelude_is_reported_without_a_source_switch() {
    let (source_a, state_a) = spawn_upstream(
        "a-key",
        vec![Reply::Stream {
            chunks: vec![
                StreamChunk::Data("data: {\"type\":\"response.created\"}\n\n"),
                StreamChunk::Data("data: {not-"),
                StreamChunk::Data("json}\n\n"),
                StreamChunk::Data("data: [DONE]\n\n"),
            ],
            cache_control: "first",
        }],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "b-key",
        vec![Reply::Stream {
            chunks: vec![StreamChunk::Data("data: [DONE]\n\n")],
            cache_control: "winner",
        }],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 10),
            source("b", &source_b, "b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let response = request(&gateway, true).await;
    assert_eq!(response.headers()[CACHE_CONTROL], "first");
    let body = response.text().await.unwrap();
    assert!(body.contains("response.created"));
    assert!(body.contains("data: {not-"));
    assert!(body.contains("response.failed"));
    assert!(!body.contains("[DONE]"));
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert!(state_b.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(events[0].error_category.as_deref(), Some("stream_invalid"));
}

#[tokio::test]
async fn chat_completions_stays_on_a_matching_chat_source_and_rejects_tool_use() {
    let chat = json!({
        "id": "chat-1",
        "object": "chat.completion",
        "created": 123,
        "model": "chat-model",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "translated"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 2, "completion_tokens": 3, "total_tokens": 5}
    });
    let (chat_server, state) = spawn_upstream(
        "chat-key",
        vec![
            Reply::Json {
                status: StatusCode::OK,
                body: chat.clone(),
                cache_control: "chat",
                retry_after: None,
            },
            Reply::Json {
                status: StatusCode::OK,
                body: chat,
                cache_control: "chat",
                retry_after: None,
            },
        ],
    )
    .await;
    let mut chat_source = source("chat", &chat_server, "chat-key", &["chat-model"], 0);
    chat_source.source.wire_api = WireApi::ChatCompletions;
    let (gateway, events) = spawn_gateway(
        vec![chat_source],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    assert_eq!(models(&gateway, LOCAL_KEY).await, ["chat-model"]);
    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "chat-model",
            "messages": [{"role": "user", "content": "hello"}],
            "service_tier": "priority"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["object"], "chat.completion");
    assert_eq!(body["choices"][0]["message"]["content"], "translated");
    assert_eq!(body["usage"]["prompt_tokens"], 2);

    {
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests
            .iter()
            .all(|request| request.path == "/v1/chat/completions"));
        assert!(requests
            .iter()
            .all(|request| request.authorization.as_deref() == Some("Bearer chat-key")));
        assert_eq!(requests[0].body["messages"][0]["content"], "hello");
        assert_eq!(requests[0].body["service_tier"], "priority");
    }
    {
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events.iter().all(|event| event.success));
        assert!(events
            .iter()
            .all(|event| event.wire_api == WireApi::ChatCompletions));
    }

    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "chat-model",
            "messages": [{
                "role": "user",
                "content": [
                    {"type": "text", "text": "What is shown?"},
                    {"type": "image_url", "image_url": {"url": "https://example.test/image.png"}}
                ]
            }]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": "chat-model", "input": "hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "chat-model",
            "messages": [{"role": "user", "content": "hello"}],
            "tools": [{"type": "function", "function": {"name": "shell"}}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "tool_use_not_supported");

    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "chat-model",
            "modalities": ["text", "audio"],
            "messages": [{"role": "user", "content": "hello"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "chat_feature_not_supported");

    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "chat-model",
            "messages": [{"role": "tool", "tool_call_id": "call_1", "content": "result"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "tool_use_not_supported");
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].body["messages"][0]["content"][1]["type"],
        "image_url"
    );
}

#[tokio::test]
async fn messages_passthrough_preserves_native_tool_use_headers_and_sse() {
    let native_message = json!({
        "id": "msg_01",
        "type": "message",
        "role": "assistant",
        "model": MODEL,
        "content": [
            {"type": "text", "text": "I will continue."},
            {
                "type": "tool_use",
                "id": "toolu_2",
                "name": "PowerShell",
                "input": {"command": "pwd"}
            }
        ],
        "stop_reason": "tool_use",
        "stop_sequence": null,
        "usage": {"input_tokens": 12, "output_tokens": 7}
    });
    let native_sse = concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_02\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"gpt-p2\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":12,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_3\",\"name\":\"PowerShell\",\"input\":{}}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":7}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n"
    );
    let (messages_server, state) = spawn_upstream(
        "messages-key",
        vec![
            Reply::Json {
                status: StatusCode::OK,
                body: native_message.clone(),
                cache_control: "messages",
                retry_after: None,
            },
            Reply::Stream {
                chunks: vec![StreamChunk::Data(native_sse)],
                cache_control: "messages",
            },
        ],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![source_with_protocol(
            "messages",
            &messages_server,
            "messages-key",
            &[MODEL],
            0,
            WireApi::Messages,
        )],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;
    let request = json!({
        "model": MODEL,
        "max_tokens": 1024,
        "system": "Use the supplied tools.",
        "messages": [
            {"role": "user", "content": "Inspect the workspace."},
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "toolu_1", "name": "PowerShell", "input": {"command": "ls"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "content": "Cargo.toml"},
                {"type": "text", "text": "Continue."}
            ]}
        ],
        "tools": [{"name": "PowerShell", "input_schema": {"type": "object"}}]
    });

    let response = reqwest::Client::new()
        .post(format!("{}/v1/messages", gateway.base_url))
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["type"], "error");
    assert_eq!(body["error"]["type"], "authentication_error");

    let response = reqwest::Client::new()
        .post(format!("{}/v1/messages", gateway.base_url))
        .header("x-api-key", LOCAL_KEY)
        .header("anthropic-beta", "fine-grained-tool-streaming-2025-05-14")
        .header("x-claude-code-session-id", "claude-session-1")
        .header("user-agent", "claude-code-test")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body, native_message);

    let mut streaming_request = request.clone();
    streaming_request["stream"] = Value::Bool(true);
    let response = reqwest::Client::new()
        .post(format!("{}/v1/messages", gateway.base_url))
        .header("x-api-key", LOCAL_KEY)
        .header("anthropic-beta", "fine-grained-tool-streaming-2025-05-14")
        .header("x-claude-code-session-id", "claude-session-1")
        .header("user-agent", "claude-code-test")
        .json(&streaming_request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "text/event-stream");
    let stream = response.text().await.unwrap();
    assert_eq!(stream, native_sse);

    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "messages": [{"role": "user", "content": "hello"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests
        .iter()
        .all(|request| request.path == "/v1/messages"));
    assert!(requests
        .iter()
        .all(|request| request.authorization.is_none()));
    assert!(requests
        .iter()
        .all(|request| request.x_api_key.as_deref() == Some("messages-key")));
    assert!(requests.iter().all(|request| {
        request.anthropic_version.as_deref() == Some("2023-06-01")
            && request.anthropic_beta.as_deref() == Some("fine-grained-tool-streaming-2025-05-14")
            && request.claude_code_session_id.as_deref() == Some("claude-session-1")
    }));
    assert_eq!(requests[0].body, request);
    assert_eq!(requests[1].body, streaming_request);
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
    assert!(events
        .iter()
        .all(|event| event.wire_api == WireApi::Messages));
    assert!(events.iter().all(|event| {
        event.tool_use.client_tool_count == 1 && event.tool_use.forwarded_tool_count == 1
    }));
}

#[tokio::test]
async fn protocol_bindings_route_each_model_only_through_its_native_endpoint() {
    let (upstream, state) = spawn_upstream("source-key", Vec::new()).await;
    let mut mixed = source(
        "mixed",
        &upstream,
        "source-key",
        &["gpt-5.4", "gpt-5.4-mini", "shared-model"],
        0,
    );
    mixed.protocol_bindings = vec![
        SourceProtocolBinding {
            wire_api: WireApi::Responses,
            model_ids: vec!["gpt-5.4".into(), "shared-model".into()],
        },
        SourceProtocolBinding {
            wire_api: WireApi::Messages,
            model_ids: vec!["gpt-5.4-mini".into(), "shared-model".into()],
        },
    ];
    let (gateway, events) =
        spawn_gateway(vec![mixed], vec![local_key("key", LOCAL_KEY, None)], 3).await;

    assert_eq!(
        models(&gateway, LOCAL_KEY).await,
        ["gpt-5.4", "shared-model"]
    );

    let catalog: Value = reqwest::Client::new()
        .get(format!(
            "{}/v1/models?client_version=1.97.0",
            gateway.base_url
        ))
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let catalog_models = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|model| model["slug"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    assert_eq!(
        catalog_models,
        [
            zenith_relay_core::codex_model_alias("gpt-5.4"),
            zenith_relay_core::codex_model_alias("shared-model"),
        ]
    );
    assert!(!catalog_models.contains(&zenith_relay_core::codex_model_alias("gpt-5.4-mini")));

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": "shared-model", "input": "hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = reqwest::Client::new()
        .post(format!("{}/v1/messages", gateway.base_url))
        .header("x-api-key", LOCAL_KEY)
        .json(&json!({
            "model": "shared-model",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "hello"}],
            "tools": [{"name": "PowerShell", "input_schema": {"type": "object"}}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": "gpt-5.4-mini", "input": "must not route"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let response = reqwest::Client::new()
        .post(format!("{}/v1/messages", gateway.base_url))
        .header("x-api-key", LOCAL_KEY)
        .json(&json!({
            "model": "gpt-5.4",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "must not route"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].path, "/v1/models");
    assert_eq!(requests[1].path, "/v1/responses");
    assert_eq!(requests[2].path, "/v1/messages");
    assert_eq!(
        requests[2].body["tools"][0]["name"].as_str(),
        Some("PowerShell")
    );
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().any(|event| {
        event.wire_api == WireApi::Responses
            && event.candidate_id.as_deref() == Some("mixed::responses")
    }));
    assert!(events.iter().any(|event| {
        event.wire_api == WireApi::Messages
            && event.candidate_id.as_deref() == Some("mixed::messages")
    }));
}

#[tokio::test]
async fn legacy_single_protocol_source_keeps_its_physical_candidate_id() {
    let (upstream, _) = spawn_upstream("source-key", Vec::new()).await;
    let (gateway, events) = spawn_gateway(
        vec![source(
            "legacy-source",
            &upstream,
            "source-key",
            &[MODEL],
            0,
        )],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].candidate_id.as_deref(), Some("legacy-source"));
    assert_eq!(events[0].wire_api, WireApi::Responses);
}

#[tokio::test]
async fn repeated_session_id_does_not_pin_requests_to_one_source() {
    let (source_a, state_a) = spawn_upstream(
        "a-key",
        vec![
            status_reply(StatusCode::TOO_MANY_REQUESTS, "a-limited", Some("0")),
            response_reply("a-rotated", "a-ready"),
        ],
    )
    .await;
    let (source_b, state_b) = spawn_upstream("b-key", vec![response_reply("b-first", "b")]).await;
    let (gateway, _) = spawn_gateway_with_options(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 10),
            source("b", &source_b, "b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        GatewayRuntimeOptions {
            max_retry_candidates: 3,
            routing_strategy: Default::default(),
            subscription_plan_order: Vec::new(),
            hidden_models: Vec::new(),
            default_service_tier: Default::default(),
            quota_stale_after_ms: zenith_relay_core::QUOTA_STALE_AFTER_MS,
            image_base_model: None,
            response_affinity_store: None,
            provider_storm_breaker: false,
        },
    )
    .await;

    assert_eq!(
        request_with_session(&gateway, "session-1")
            .await
            .json::<Value>()
            .await
            .unwrap()["id"],
        "b-first"
    );
    assert_eq!(
        request_with_session(&gateway, "session-1")
            .await
            .json::<Value>()
            .await
            .unwrap()["id"],
        "a-rotated"
    );
    assert_eq!(state_a.requests.lock().unwrap().len(), 2);
    assert_eq!(state_b.requests.lock().unwrap().len(), 1);
}

fn source(
    id: &str,
    server: &TestServer,
    key: &str,
    models: &[&str],
    priority: i32,
) -> RuntimeSource {
    RuntimeSource {
        source: ProviderSource {
            id: id.to_string(),
            name: id.to_string(),
            base_url: format!("{}/v1", server.base_url),
            api_key: key.to_string(),
            wire_api: WireApi::Responses,
            models: models.iter().map(|model| (*model).to_string()).collect(),
        },
        protocol_bindings: Vec::new(),
        enabled: true,
        draining: false,
        priority,
        weight: 1,
        recovery_delay_seconds: 0,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        last_used_at_ms: None,
    }
}

fn source_with_protocol(
    id: &str,
    server: &TestServer,
    key: &str,
    models: &[&str],
    priority: i32,
    wire_api: WireApi,
) -> RuntimeSource {
    let mut source = source(id, server, key, models, priority);
    source.source.wire_api = wire_api;
    source.protocol_bindings = vec![SourceProtocolBinding::legacy(
        wire_api,
        &source.source.models,
    )];
    source
}

fn local_key(id: &str, secret: &str, source_ids: Option<Vec<&str>>) -> RuntimeLocalKey {
    RuntimeLocalKey {
        key: LocalGatewayKey {
            id: id.to_string(),
            secret: secret.to_string(),
        },
        enabled: true,
        source_ids: source_ids.map(|ids| ids.into_iter().map(str::to_string).collect()),
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        model_prefix: None,
    }
}

async fn spawn_gateway(
    sources: Vec<RuntimeSource>,
    keys: Vec<RuntimeLocalKey>,
    max_retry_candidates: usize,
) -> (TestServer, Arc<Mutex<Vec<UsageEvent>>>) {
    spawn_gateway_with_options(
        sources,
        keys,
        GatewayRuntimeOptions {
            max_retry_candidates,
            routing_strategy: Default::default(),
            subscription_plan_order: Vec::new(),
            hidden_models: Vec::new(),
            default_service_tier: Default::default(),
            quota_stale_after_ms: zenith_relay_core::QUOTA_STALE_AFTER_MS,
            image_base_model: None,
            response_affinity_store: None,
            provider_storm_breaker: false,
        },
    )
    .await
}

async fn spawn_gateway_with_options(
    sources: Vec<RuntimeSource>,
    keys: Vec<RuntimeLocalKey>,
    options: GatewayRuntimeOptions,
) -> (TestServer, Arc<Mutex<Vec<UsageEvent>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let runtime = GatewayRuntime::from_pool(
        sources,
        keys,
        options,
        Arc::new(move |event| captured.lock().unwrap().push(event)),
    )
    .unwrap();
    (spawn(gateway::router(Arc::new(runtime))).await, events)
}

async fn spawn_upstream(key: &str, replies: Vec<Reply>) -> (TestServer, UpstreamState) {
    let state = UpstreamState {
        key: key.to_string(),
        replies: Arc::new(Mutex::new(replies.into())),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let router = Router::new()
        .route("/v1/models", get(upstream))
        .route("/v1/responses", post(upstream))
        .route("/v1/chat/completions", post(upstream))
        .route("/v1/messages", post(upstream))
        .with_state(state.clone());
    (spawn(router).await, state)
}

async fn spawn(app: Router) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        base_url: format!("http://{address}"),
        task,
    }
}

async fn upstream(
    State(state): State<UpstreamState>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    let parsed_body = serde_json::from_slice(&body).unwrap_or(Value::Null);
    state.requests.lock().unwrap().push(ObservedRequest {
        path: uri.path().to_string(),
        authorization: headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        x_api_key: headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        anthropic_version: headers
            .get("anthropic-version")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        anthropic_beta: headers
            .get("anthropic-beta")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        claude_code_session_id: headers
            .get("x-claude-code-session-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string),
        body: parsed_body,
    });
    let authorized = if uri.path() == "/v1/messages" {
        headers
            .get("x-api-key")
            .and_then(|value| value.to_str().ok())
            == Some(state.key.as_str())
    } else {
        let expected = format!("Bearer {}", state.key);
        headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            == Some(expected.as_str())
    };
    if !authorized {
        return Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .body(Body::empty())
            .unwrap();
    }

    let reply = state
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| response_reply("default-response", "default"));
    match reply {
        Reply::Json {
            status,
            body,
            cache_control,
            retry_after,
        } => {
            let mut response = Response::builder()
                .status(status)
                .header(CONTENT_TYPE, "application/json")
                .header(CACHE_CONTROL, cache_control);
            if let Some(retry_after) = retry_after {
                response = response.header("retry-after", retry_after);
            }
            response.body(Body::from(body.to_string())).unwrap()
        }
        Reply::Stream {
            chunks,
            cache_control,
        } => {
            let chunks = stream::unfold(VecDeque::from(chunks), |mut chunks| async move {
                let chunk = chunks.pop_front()?;
                tokio::time::sleep(Duration::from_millis(10)).await;
                let item = match chunk {
                    StreamChunk::Data(data) => {
                        Ok::<_, io::Error>(Bytes::from_static(data.as_bytes()))
                    }
                    StreamChunk::Error => Err(io::Error::other("synthetic stream failure")),
                };
                Some((item, chunks))
            });
            Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, "text/event-stream")
                .header(CACHE_CONTROL, cache_control)
                .body(Body::from_stream(chunks))
                .unwrap()
        }
    }
}

fn status_reply(
    status: StatusCode,
    cache_control: &'static str,
    retry_after: Option<&'static str>,
) -> Reply {
    Reply::Json {
        status,
        body: json!({"error": {"message": status.as_str()}}),
        cache_control,
        retry_after,
    }
}

fn response_reply(id: &str, cache_control: &'static str) -> Reply {
    Reply::Json {
        status: StatusCode::OK,
        body: json!({
            "id": id,
            "object": "response",
            "model": MODEL,
            "output": [],
            "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
        }),
        cache_control,
        retry_after: None,
    }
}

async fn request(gateway: &TestServer, stream: bool) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "hello", "stream": stream}))
        .send()
        .await
        .unwrap()
}

async fn request_with_session(gateway: &TestServer, session: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("x-session-id", session)
        .json(&json!({"model": MODEL, "input": "hello"}))
        .send()
        .await
        .unwrap()
}

async fn models(gateway: &TestServer, key: &str) -> Vec<String> {
    let body: Value = reqwest::Client::new()
        .get(format!("{}/v1/models", gateway.base_url))
        .bearer_auth(key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    body["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|model| model["id"].as_str().map(str::to_string))
        .collect()
}

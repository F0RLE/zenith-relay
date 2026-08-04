use axum::body::{Body, Bytes};
use axum::extract::ws::{Message as AxumWsMessage, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, Response, StatusCode, Uri};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::future::{join_all, BoxFuture};
use futures_util::stream;
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{Message as ClientWsMessage, Upgrade};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use zenith_relay_core::accounts::{
    AccountAuthState, ReauthReason, TokenAuthority, TokenPersistenceAdapter,
    TokenPersistenceFailure, TokenRefresh, TokenRefreshAdapter, TokenRefreshFailure, TokenSet,
};
use zenith_relay_core::gateway;
use zenith_relay_core::providers::chatgpt::{AgentIdentityCredential, CODEX_MODELS_CLIENT_VERSION};
use zenith_relay_core::{
    CandidateHealth, CandidateQuota, DefaultServiceTier, GatewayRuntime, GatewayRuntimeOptions,
    LocalGatewayKey, ProviderSource, RuntimeChatGptAccount, RuntimeChatGptAuth,
    RuntimeMixedLocalKey, RuntimeSource, SelectionReason, UsageEvent, WireApi,
};

const LOCAL_KEY: &str = "p3-local-key";
const MODEL: &str = "gpt-p3";

#[derive(Clone, Debug)]
struct ObservedRequest {
    path: String,
    authorization: Option<String>,
    chatgpt_account_id: Option<String>,
    originator: Option<String>,
    responses_lite: Option<String>,
    session_id: Option<String>,
    body: Value,
}

#[derive(Clone)]
enum Reply {
    Json(StatusCode, Value),
    JsonWithHeaders(StatusCode, Value, Vec<(&'static str, &'static str)>),
    Stream(Vec<StreamChunk>),
}

#[derive(Clone)]
enum StreamChunk {
    Data(&'static str),
    Error,
}

#[derive(Clone, Default)]
struct UpstreamState {
    replies: Arc<Mutex<VecDeque<Reply>>>,
    requests: Arc<Mutex<Vec<ObservedRequest>>>,
    delay: Duration,
    model_catalog: Value,
}

#[derive(Clone, Default)]
struct HeldStreamState {
    requests: Arc<Mutex<Vec<ObservedRequest>>>,
    release: Arc<Notify>,
}

#[derive(Clone, Default)]
struct ConnectionAffinityState {
    owners: Arc<Mutex<HashMap<SocketAddr, String>>>,
    account_ids: Arc<Mutex<Vec<String>>>,
}

#[derive(Clone, Default)]
struct WebSocketUpstreamState {
    headers: Arc<Mutex<Vec<HeaderMap>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    behavior: WebSocketBehavior,
}

#[derive(Clone, Default)]
enum WebSocketBehavior {
    #[default]
    Success,
    Events(Arc<Vec<Value>>),
    Sequence(Arc<Mutex<VecDeque<Vec<Value>>>>),
    Hold(Arc<Notify>),
    Close,
    OutputThenClose,
    UnauthorizedOnce(Arc<AtomicUsize>),
}

struct TestServer {
    base_url: String,
    task: JoinHandle<()>,
    runtime: Option<Arc<GatewayRuntime>>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct RefreshAdapter {
    calls: AtomicUsize,
    delay: Duration,
    access_token: &'static str,
}

impl TokenRefreshAdapter for RefreshAdapter {
    fn refresh<'a>(
        &'a self,
        account_id: &'a str,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async move {
            assert_eq!(account_id, "relay-refresh-account");
            assert_eq!(refresh_token, "refresh-secret");
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            TokenRefresh::new(self.access_token, None, None, Some(now_ms + 60_000))
                .map_err(|_| unreachable!())
        })
    }
}

#[derive(Default)]
struct PersistenceAdapter {
    token_writes: AtomicUsize,
    persisted_accounts: Mutex<Vec<String>>,
    auth_states: Mutex<Vec<(String, AccountAuthState)>>,
}

impl TokenPersistenceAdapter for PersistenceAdapter {
    fn persist<'a>(
        &'a self,
        account_id: &'a str,
        _tokens: &'a TokenSet,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
        Box::pin(async move {
            self.token_writes.fetch_add(1, Ordering::SeqCst);
            self.persisted_accounts
                .lock()
                .unwrap()
                .push(account_id.to_string());
            Ok(())
        })
    }

    fn persist_auth_state<'a>(
        &'a self,
        account_id: &'a str,
        auth_state: AccountAuthState,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
        Box::pin(async move {
            self.auth_states
                .lock()
                .unwrap()
                .push((account_id.to_string(), auth_state));
            Ok(())
        })
    }

    fn persist_agent_task_id<'a>(
        &'a self,
        _account_id: &'a str,
        _expected_task_id: Option<&'a str>,
        task_id: &'a str,
    ) -> BoxFuture<'a, Result<String, TokenPersistenceFailure>> {
        Box::pin(async move { Ok(task_id.to_string()) })
    }
}

#[tokio::test]
async fn account_headers_use_provider_id_but_usage_keeps_only_local_identity() {
    let (upstream, state) = spawn_upstream(vec![success_reply("account-response")]).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let runtime_account = account("relay-account", "provider-account-private", &upstream, 10);
    assert!(!format!("{runtime_account:?}").contains("provider-account-private"));
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![runtime_account],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer account-access")
    );
    assert_eq!(
        requests[0].chatgpt_account_id.as_deref(),
        Some("provider-account-private")
    );
    assert_eq!(requests[0].originator.as_deref(), Some("codex_cli_rs"));
    assert_eq!(requests[0].body["store"], false);
    assert_eq!(requests[0].body["stream"], true);
    assert!(requests[0].body["input"].is_array());
    assert!(requests[0].body.get("max_output_tokens").is_none());
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events[0].candidate_id.as_deref(), Some("relay-account"));
    assert_eq!(events[0].account_id.as_deref(), Some("relay-account"));
    assert_eq!(events[0].source_id, "openai-codex");
    let serialized = serde_json::to_string(&events[0]).unwrap();
    assert!(!serialized.contains("provider-account-private"));
    assert!(!serialized.contains("account-access"));
}

#[tokio::test]
async fn unauthorized_account_request_refreshes_once_and_retries_the_same_account() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::UNAUTHORIZED,
            json!({"error":{"code":"token_expired"}}),
        ),
        success_reply("refreshed-response"),
    ])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    authority
        .register(
            "relay-refresh-account",
            TokenSet::new(
                "old-access",
                Some("refresh-secret".into()),
                None,
                Some(current_time_ms() + 600_000),
                current_time_ms(),
                1,
            )
            .unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let refresh = Arc::new(RefreshAdapter {
        calls: AtomicUsize::new(0),
        delay: Duration::ZERO,
        access_token: "new-access",
    });
    let (gateway, events, refresh, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "relay-refresh-account",
            "provider-refresh-account",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh,
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer old-access")
    );
    assert_eq!(
        requests[1].authorization.as_deref(),
        Some("Bearer new-access")
    );
    assert!(requests.iter().all(|request| {
        request.chatgpt_account_id.as_deref() == Some("provider-refresh-account")
    }));
    drop(requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
}

#[tokio::test]
async fn passive_quota_headers_update_the_active_runtime_before_persistence() {
    let (upstream, _) = spawn_upstream(vec![Reply::JsonWithHeaders(
        StatusCode::OK,
        json!({
            "id":"quota-response",
            "object":"response",
            "model":MODEL,
            "output":[],
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
        }),
        vec![
            ("x-codex-primary-used-percent", "100"),
            ("x-codex-primary-reset-after-seconds", "60"),
        ],
    )])
    .await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(
        request(&gateway, false).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    let quota = events[0].quota_snapshot.as_ref().unwrap();
    assert!(quota.limit_reached);
    assert_eq!(
        quota.primary.as_ref().unwrap().available_basis_points,
        Some(0)
    );
}

#[tokio::test]
async fn invalid_encrypted_reasoning_is_stripped_once_before_semantic_output() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::BAD_REQUEST,
            json!({"error":{"code":"invalid_encrypted_content"}}),
        ),
        success_reply("recovered-response"),
    ])
    .await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": [
                {"id":"rs_1","type":"reasoning","encrypted_content":"invalid","summary":[]},
                {"role":"user","content":"continue"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].body["input"][0]["encrypted_content"], "invalid");
    assert!(requests[1]
        .body
        .pointer("/input/0/encrypted_content")
        .is_none());
    assert!(requests[1].body.pointer("/input/0/id").is_none());
    drop(requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_encrypted_content_invalid")
    );
    assert!(events[1].success);
}

#[tokio::test]
async fn image_generation_uses_cheapest_account_model_and_translates_response() {
    let (upstream, state) = spawn_upstream(vec![Reply::Stream(vec![
        StreamChunk::Data(
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"image_generation_call\",\"result\":\"aW1hZ2U=\",\"output_format\":\"png\"}}\n\n",
        ),
        StreamChunk::Data(
            "data: {\"type\":\"response.completed\",\"response\":{\"created_at\":7,\"output\":[],\"tool_usage\":{\"image_gen\":{\"image_tokens\":4}}}}\n\n",
        ),
    ])])
    .await;
    let authority = ready_authority("relay-image-account", "image-access").await;
    let mut image_account = account(
        "relay-image-account",
        "provider-image-account",
        &upstream,
        10,
    );
    image_account.models.push("gpt-5.6-terra".to_string());
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![image_account],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/images/generations", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":"gpt-image-2","prompt":"draw a test"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["data"][0]["b64_json"], "aW1hZ2U=");

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/responses");
    assert_eq!(requests[0].body["model"], "gpt-5.6-terra");
    assert_eq!(requests[0].body["tools"][0]["type"], "image_generation");
    assert_eq!(requests[0].body["tools"][0]["model"], "gpt-image-2");
    assert!(requests[0].body["tools"][0].get("size").is_none());
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].requested_model.as_deref(), Some("gpt-image-2"));
    assert_eq!(events[0].resolved_model.as_deref(), Some("gpt-5.6-terra"));
}

#[tokio::test]
async fn codex_catalog_exposes_and_forwards_confirmed_native_tiers_reasoning_and_parallel_tools() {
    let (upstream, state) = spawn_upstream(Vec::new()).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let catalog: Value = reqwest::Client::new()
        .get(format!(
            "{}/v1/models?client_version=26.707.8479.0",
            gateway.base_url
        ))
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .any(|model| model["slug"] == MODEL));
    assert_eq!(catalog["models"][0]["service_tiers"][0]["id"], "priority");
    assert_eq!(catalog["models"][0]["use_responses_lite"], true);
    assert_eq!(catalog["models"][0]["supports_parallel_tool_calls"], true);
    assert_eq!(catalog["models"][0]["default_reasoning_level"], "high");
    assert_eq!(
        catalog["models"][0]["supported_reasoning_levels"],
        json!([
            {"effort": "low", "description": "Low"},
            {"effort": "high", "description": "High"},
            {"effort": "xhigh", "description": "Extra high"}
        ])
    );
    assert_eq!(
        catalog["models"][0]["supports_reasoning_summary_parameter"],
        true
    );
    assert_eq!(catalog["models"][0]["supports_reasoning_summaries"], true);
    assert_eq!(
        catalog["models"][0]["default_reasoning_summary"],
        "detailed"
    );

    // The pool may classify a tier for quota telemetry, but must never
    // translate a ChatGPT/Codex client's native service-tier selection.
    // `fast` remains here as a legacy client value that must also pass through
    // unchanged; the current native values must remain unchanged as well.
    let client_tiers = [
        None,
        Some("standard"),
        Some("default"),
        Some("flex"),
        Some("priority"),
        Some("fast"),
    ];
    for service_tier in client_tiers {
        let mut body = json!({
            "model": MODEL,
            "input": "hello",
            "parallel_tool_calls": true,
            "reasoning": {
                "effort": "xhigh",
                "summary": "detailed"
            }
        });
        if let Some(service_tier) = service_tier {
            body["service_tier"] = Value::String(service_tier.to_string());
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
    assert_eq!(requests.len(), 2 + client_tiers.len());
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer account-access")
    );
    assert_eq!(
        requests[0].chatgpt_account_id.as_deref(),
        Some("provider-account")
    );
    assert_eq!(requests[0].body["client_version"], "26.707.8479.0");
    assert_eq!(
        requests[1].body["client_version"],
        CODEX_MODELS_CLIENT_VERSION
    );
    for (request, expected_tier) in requests[2..].iter().zip(client_tiers) {
        assert_eq!(
            request.body.get("service_tier").and_then(Value::as_str),
            expected_tier
        );
        assert_eq!(request.responses_lite.as_deref(), Some("true"));
        assert_eq!(request.body["parallel_tool_calls"], true);
        assert_eq!(request.body["reasoning"]["effort"], "xhigh");
        assert_eq!(request.body["reasoning"]["summary"], "detailed");
        assert_eq!(request.body["reasoning"]["context"], "all_turns");
    }
}

#[tokio::test]
async fn pool_catalog_combines_native_metadata_from_each_available_account() {
    let (first_upstream, first_state) =
        spawn_upstream_with_catalog(Vec::new(), json!({"models": []})).await;
    let (second_upstream, second_state) =
        spawn_upstream_with_catalog(Vec::new(), default_upstream_model_catalog()).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "first-account", "first-access").await;
    register_ready(&authority, "second-account", "second-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &first_upstream, 100),
            account("second-account", "provider-second", &second_upstream, 10),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let catalog: Value = reqwest::Client::new()
        .get(format!(
            "{}/v1/models?client_version={CODEX_MODELS_CLIENT_VERSION}",
            gateway.base_url
        ))
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let model = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["slug"] == MODEL)
        .unwrap();
    assert_eq!(model["default_reasoning_level"], "high");
    assert_eq!(model["supported_reasoning_levels"][2]["effort"], "xhigh");
    assert_eq!(model["service_tiers"][0]["id"], "priority");
    assert_eq!(model["supports_reasoning_summaries"], true);
    assert_eq!(first_state.requests.lock().unwrap().len(), 1);
    assert_eq!(second_state.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn pool_catalog_retains_unreachable_account_metadata_beside_live_account_catalogs() {
    let (first_upstream, first_state) =
        spawn_upstream_with_catalog(Vec::new(), json!({"models": []})).await;
    let (second_upstream, second_state) =
        spawn_upstream_with_catalog(Vec::new(), default_upstream_model_catalog()).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "first-account", "first-access").await;
    register_ready(&authority, "second-account", "second-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &first_upstream, 100),
            account("second-account", "provider-second", &second_upstream, 10),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let url = format!(
        "{}/v1/models?client_version={CODEX_MODELS_CLIENT_VERSION}",
        gateway.base_url
    );
    let client = reqwest::Client::new();
    let live: Value = client
        .get(&url)
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(live["models"][0]["default_reasoning_level"], "high");
    assert_eq!(live["models"][0]["supports_parallel_tool_calls"], true);

    drop(second_upstream);

    let recovered: Value = client
        .get(&url)
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(recovered, live);
    assert_eq!(first_state.requests.lock().unwrap().len(), 2);
    assert_eq!(second_state.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn pool_catalog_keeps_native_capabilities_on_a_mixed_source_model() {
    let (source_upstream, source_state) = spawn_upstream(Vec::new()).await;
    let (account_upstream, account_state) = spawn_upstream(Vec::new()).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        vec![source(
            "generic-source",
            &source_upstream,
            "source-key",
            100,
        )],
        vec![account(
            "relay-account",
            "provider-account",
            &account_upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let catalog: Value = reqwest::Client::new()
        .get(format!(
            "{}/v1/models?client_version={CODEX_MODELS_CLIENT_VERSION}",
            gateway.base_url
        ))
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    let model = catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["slug"] == MODEL)
        .unwrap();
    assert_eq!(model["default_reasoning_level"], "high");
    assert_eq!(model["supported_reasoning_levels"][2]["effort"], "xhigh");
    assert_eq!(model["service_tiers"][0]["id"], "priority");
    assert_eq!(model["use_responses_lite"], true);
    assert_eq!(model["supports_reasoning_summaries"], true);

    let source_requests_before = source_state.requests.lock().unwrap().len();
    let account_requests_before = account_state.requests.lock().unwrap().len();
    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        source_state.requests.lock().unwrap().len(),
        source_requests_before
    );
    assert_eq!(
        account_state.requests.lock().unwrap().len(),
        account_requests_before + 1
    );
}

#[tokio::test]
async fn codex_catalog_uses_the_last_manifest_when_live_discovery_is_unavailable() {
    let (upstream, _) = spawn_upstream(Vec::new()).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let url = format!(
        "{}/v1/models?client_version={CODEX_MODELS_CLIENT_VERSION}",
        gateway.base_url
    );
    let client = reqwest::Client::new();
    let live: Value = client
        .get(&url)
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(live["models"][0]["slug"], MODEL);
    drop(upstream);

    let stale: Value = client
        .get(&url)
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stale, live);
}

#[tokio::test]
async fn codex_catalog_prefers_a_usable_account_token() {
    let (upstream, state) = spawn_upstream(Vec::new()).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    authority
        .register(
            "stale-account",
            TokenSet::access_only("stale-access", Some(1), 0).unwrap(),
            AccountAuthState::RequiresReauth(ReauthReason::ExpiredRefreshToken),
        )
        .await
        .unwrap();
    register_ready(&authority, "ready-account", "ready-access").await;
    let mut stale = account("stale-account", "stale-provider", &upstream, 10);
    stale.models.push("gpt-extra".to_string());
    let ready = account("ready-account", "ready-provider", &upstream, 10);
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![stale, ready],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let catalog: Value = reqwest::Client::new()
        .get(format!(
            "{}/v1/models?client_version={CODEX_MODELS_CLIENT_VERSION}",
            gateway.base_url,
        ))
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert!(catalog["models"]
        .as_array()
        .unwrap()
        .iter()
        .any(|model| model["slug"] == MODEL));
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer ready-access")
    );
}

#[tokio::test]
async fn account_requests_preserve_responses_lite_compatibility() {
    let (upstream, state) = spawn_upstream(vec![success_reply("lite-response")]).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("x-openai-internal-codex-responses-lite", "true")
        .json(&json!({
            "model": MODEL,
            "input": "hello",
            "parallel_tool_calls": true,
            "reasoning": {"effort": "high"},
            "tools": [
                {"type": "function", "name": "local_tool"},
                {"type": "web_search"},
                {"type": "image_generation"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests[0].responses_lite.as_deref(), Some("true"));
    assert_eq!(requests[0].body["parallel_tool_calls"], true);
    assert_eq!(requests[0].body["tools"].as_array().unwrap().len(), 1);
    assert_eq!(requests[0].body["tools"][0]["name"], "local_tool");
    assert_eq!(requests[0].body["reasoning"]["context"], "all_turns");
    assert_eq!(requests[0].body["reasoning"]["effort"], "high");
}

#[tokio::test]
async fn compact_and_alpha_search_use_the_oauth_account_runtime() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(StatusCode::OK, json!({"type": "compaction", "items": []})),
        Reply::Json(StatusCode::OK, json!({"results": [{"title": "result"}]})),
    ])
    .await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let client = reqwest::Client::new();

    let compact = client
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("x-openai-internal-codex-responses-lite", "true")
        .json(&json!({
            "model": MODEL,
            "input": "compact this",
            "stream": false,
            "max_output_tokens": 4
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(compact.status(), StatusCode::OK);
    assert_eq!(compact.json::<Value>().await.unwrap()["type"], "compaction");

    let search = client
        .post(format!("{}/v1/alpha/search", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("user-agent", "synthetic-codex")
        .json(&json!({
            "model": MODEL,
            "id": "session-42",
            "query": "search query",
            "prompt_cache_key": "local-only",
            "prompt_cache_retention": "24h"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(search.status(), StatusCode::OK);
    assert_eq!(
        search.json::<Value>().await.unwrap()["results"][0]["title"],
        "result"
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/v1/responses/compact");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer account-access")
    );
    assert_eq!(requests[0].responses_lite.as_deref(), Some("true"));
    assert!(requests[0].body.get("stream").is_none());
    assert!(requests[0].body.get("max_output_tokens").is_none());
    assert_eq!(requests[1].path, "/v1/alpha/search");
    assert_eq!(requests[1].session_id.as_deref(), Some("session-42"));
    assert!(requests[1].body.get("prompt_cache_key").is_none());
    assert!(requests[1].body.get("prompt_cache_retention").is_none());
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
    assert!(events
        .iter()
        .all(|event| event.account_id.as_deref() == Some("relay-account")));
}

#[tokio::test]
async fn codex_compatibility_aliases_reach_the_canonical_account_endpoints() {
    let (upstream, state) = spawn_upstream(vec![
        success_reply("alias-response"),
        Reply::Json(StatusCode::OK, json!({"type": "compaction"})),
        Reply::Json(StatusCode::OK, json!({"results": []})),
    ])
    .await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "{}/v1/chat/completions/v1/responses",
            gateway.base_url
        ))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let compact = client
        .post(format!(
            "{}/v1/chat/completions/v1/responses/compact",
            gateway.base_url
        ))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(compact.status(), StatusCode::OK);
    let search = client
        .post(format!(
            "{}/backend-api/codex/alpha/search",
            gateway.base_url
        ))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "query": "hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(search.status(), StatusCode::OK);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests[0].path, "/v1/responses");
    assert_eq!(requests[1].path, "/v1/responses/compact");
    assert_eq!(requests[2].path, "/v1/alpha/search");
}

#[tokio::test]
async fn account_only_endpoints_never_forward_to_an_api_key_source() {
    let (upstream, state) = spawn_upstream(Vec::new()).await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("api-source", &upstream, "source-secret", 100)],
        Vec::new(),
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(1).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let client = reqwest::Client::new();
    for path in ["/v1/responses/compact", "/v1/alpha/search"] {
        let response = client
            .post(format!("{}{path}", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model": MODEL, "input": "hello", "query": "hello"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    assert!(state.requests.lock().unwrap().is_empty());
    assert!(events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn previous_response_id_keeps_http_continuations_on_the_creating_account() {
    let (first_upstream, first_state) = spawn_upstream(vec![
        success_reply("first-response"),
        success_reply("first-continuation"),
    ])
    .await;
    let (second_upstream, second_state) = spawn_upstream(vec![
        success_reply("second-response"),
        success_reply("second-continuation"),
    ])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "first-account", "first-access").await;
    register_ready(&authority, "second-account", "second-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &first_upstream, 100),
            account("second-account", "provider-second", &second_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let client = reqwest::Client::new();
    let first: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "start"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let response_id = first["id"].as_str().unwrap();
    let continued = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "continue",
            "previous_response_id": response_id
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(continued.status(), StatusCode::OK);

    let counts = [
        first_state.requests.lock().unwrap().len(),
        second_state.requests.lock().unwrap().len(),
    ];
    assert!(counts == [2, 0] || counts == [0, 2]);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].candidate_id, events[1].candidate_id);
}

#[tokio::test]
async fn prompt_cache_key_keeps_sequential_http_requests_on_the_same_account() {
    let (first_upstream, first_state) = spawn_upstream(vec![
        success_reply("first-response"),
        success_reply("first-continuation"),
    ])
    .await;
    let (second_upstream, second_state) = spawn_upstream(vec![
        success_reply("second-response"),
        success_reply("second-continuation"),
    ])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "first-account", "first-access").await;
    register_ready(&authority, "second-account", "second-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &first_upstream, 100),
            account("second-account", "provider-second", &second_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let client = reqwest::Client::new();
    for input in ["start", "continue"] {
        let response = client
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({
                "model": MODEL,
                "input": input,
                "prompt_cache_key": "thread-1"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let counts = [
        first_state.requests.lock().unwrap().len(),
        second_state.requests.lock().unwrap().len(),
    ];
    assert!(counts == [2, 0] || counts == [0, 2]);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].candidate_id, events[1].candidate_id);
    assert_eq!(
        events[1].routing.as_ref().map(|routing| routing.reason),
        Some(SelectionReason::PromptCacheAffinity)
    );
}

#[tokio::test]
async fn unknown_http_response_owner_is_recovered_without_cooling_wrong_accounts() {
    let (wrong_upstream, wrong_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::BAD_REQUEST,
        json!({"error": {
            "message": "Previous response with id 'response-from-before-restart' not found.",
            "type": "invalid_request_error",
            "code": "previous_response_not_found"
        }}),
    )])
    .await;
    let (owner_upstream, owner_state) = spawn_upstream(vec![
        success_reply("recovered-response"),
        success_reply("continued-response"),
    ])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "wrong-account", "wrong-access").await;
    register_ready(&authority, "owner-account", "owner-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("wrong-account", "provider-wrong", &wrong_upstream, 100),
            account("owner-account", "provider-owner", &owner_upstream, 10),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let client = reqwest::Client::new();
    let recovered: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "continue after restart",
            "previous_response_id": "response-from-before-restart"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(recovered["id"], "recovered-response");

    let continued = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "continue again",
            "previous_response_id": recovered["id"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(continued.status(), StatusCode::OK);
    assert_eq!(wrong_state.requests.lock().unwrap().len(), 1);
    assert_eq!(owner_state.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("response_affinity_miss")
    );
    assert_eq!(events[0].retry_at_ms, None);
    assert!(events[1].success);
    assert_eq!(events[1].candidate_id, events[2].candidate_id);
}

#[tokio::test]
async fn unknown_http_response_owner_does_not_retry_arbitrary_bad_requests() {
    let (wrong_upstream, wrong_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::BAD_REQUEST,
        json!({"error": {
            "message": "Invalid request body.",
            "type": "invalid_request_error",
            "code": "invalid_request"
        }}),
    )])
    .await;
    let (owner_upstream, owner_state) = spawn_upstream(vec![success_reply("must-not-run")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "wrong-account", "wrong-access").await;
    register_ready(&authority, "owner-account", "owner-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("wrong-account", "provider-wrong", &wrong_upstream, 100),
            account("owner-account", "provider-owner", &owner_upstream, 10),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "invalid continuation",
            "previous_response_id": "response-from-before-restart"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(wrong_state.requests.lock().unwrap().len(), 1);
    assert!(owner_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_invalid_request")
    );
}

#[tokio::test]
async fn orphaned_http_response_resets_once_without_tool_output() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::BAD_REQUEST,
            json!({"error": {
                "message": "Previous response with id 'orphaned-response' not found.",
                "type": "invalid_request_error",
                "code": "previous_response_not_found"
            }}),
        ),
        success_reply("fresh-response"),
    ])
    .await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response: Value = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "continue without an available response owner",
            "previous_response_id": "orphaned-response"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(response["id"], "fresh-response");
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].body["previous_response_id"],
        "orphaned-response"
    );
    assert!(requests[1].body.get("previous_response_id").is_none());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("response_affinity_miss")
    );
    assert!(events[1].success);
}

#[tokio::test]
async fn orphaned_http_response_with_tool_output_is_not_reset() {
    let (upstream, state) = spawn_upstream(vec![Reply::Json(
        StatusCode::BAD_REQUEST,
        json!({"error": {
            "message": "Previous response with id 'orphaned-response' not found.",
            "type": "invalid_request_error",
            "code": "previous_response_not_found"
        }}),
    )])
    .await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": [{
                "type": "function_call_output",
                "call_id": "call_1",
                "output": "done"
            }],
            "previous_response_id": "orphaned-response"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(state.requests.lock().unwrap().len(), 1);
    assert_eq!(events.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn stale_http_response_affinity_resets_before_quota_routing() {
    let (fallback_upstream, fallback_state) =
        spawn_upstream(vec![success_reply("fallback-response")]).await;
    let (owner_upstream, owner_state) = spawn_upstream(vec![
        success_reply("stale-response"),
        Reply::Json(
            StatusCode::BAD_REQUEST,
            json!({"error": {
                "message": "Previous response with id 'stale-response' not found.",
                "type": "invalid_request_error",
                "code": "previous_response_not_found"
            }}),
        ),
        success_reply("recovered-response"),
    ])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "owner-account", "owner-access").await;
    register_ready(&authority, "fallback-account", "fallback-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("owner-account", "provider-owner", &owner_upstream, 100),
            account(
                "fallback-account",
                "provider-fallback",
                &fallback_upstream,
                10,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let client = reqwest::Client::new();
    let first: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "start"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["id"], "stale-response");

    let recovered: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "continue after stale binding",
            "previous_response_id": first["id"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(recovered["id"], "recovered-response");
    assert_eq!(owner_state.requests.lock().unwrap().len(), 3);
    assert!(fallback_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(events[0].success);
    assert_eq!(
        events[1].error_category.as_deref(),
        Some("response_affinity_miss")
    );
    assert!(events[2].success);
    assert_eq!(events[2].candidate_id.as_deref(), Some("owner-account"));
}

#[tokio::test]
async fn account_websocket_preserves_codex_headers_and_reports_usage() {
    let (upstream, state) = spawn_websocket_upstream().await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions {
            default_service_tier: DefaultServiceTier::Fast,
            ..GatewayRuntimeOptions::default()
        },
    )
    .await;

    let rejected = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth("wrong-local-key")
        .upgrade()
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    assert!(state.headers.lock().unwrap().is_empty());

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("originator", "codex_test")
        .header("openai-beta", "existing_feature=1")
        .header("x-openai-internal-codex-responses-lite", "true")
        .upgrade()
        .send()
        .await
        .unwrap();
    assert_eq!(upgraded.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = upgraded.into_websocket().await.unwrap();

    for index in 0..2 {
        let mut request = json!({
            "type": "response.create",
            "model": MODEL,
            "input": format!("hello {index}"),
            "parallel_tool_calls": true
        });
        if index == 1 {
            request["service_tier"] = Value::String("flex".to_string());
            request["reasoning"] = json!({
                "effort": "high",
                "summary": "detailed",
                "context": "previous_turn"
            });
        }
        socket
            .send(ClientWsMessage::Text(request.to_string()))
            .await
            .unwrap();
        let completed = receive_websocket_completion(&mut socket).await;
        assert_eq!(completed["response"]["usage"]["input_tokens"], 11);
    }

    let headers = state.headers.lock().unwrap();
    assert_eq!(headers.len(), 2);
    for headers in headers.iter() {
        assert_eq!(
            header(headers, AUTHORIZATION.as_str()).as_deref(),
            Some("Bearer account-access")
        );
        assert_eq!(
            header(headers, "chatgpt-account-id").as_deref(),
            Some("provider-account")
        );
        assert_eq!(
            header(headers, "originator").as_deref(),
            Some("codex_cli_rs")
        );
        assert_eq!(
            header(headers, "x-openai-internal-codex-responses-lite").as_deref(),
            Some("true")
        );
        let beta = headers
            .get_all("openai-beta")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>()
            .join(",");
        assert!(beta.contains("existing_feature=1"));
        assert!(beta.contains("responses_websockets=2026-02-06"));
    }
    drop(headers);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["type"], "response.create");
    assert_eq!(requests[0]["store"], false);
    assert_eq!(requests[0]["stream"], true);
    assert_eq!(requests[0]["parallel_tool_calls"], true);
    assert!(requests[0].get("service_tier").is_none());
    assert_eq!(requests[1]["service_tier"], "flex");
    assert_eq!(requests[1]["reasoning"]["effort"], "high");
    assert_eq!(requests[1]["reasoning"]["summary"], "detailed");
    assert_eq!(requests[1]["reasoning"]["context"], "all_turns");
    assert!(requests[0]["input"].is_array());
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
    assert!(events.iter().all(|event| event.input_tokens == Some(11)));
    assert!(events
        .iter()
        .all(|event| event.cached_input_tokens == Some(7)));
    assert!(events.iter().all(|event| event.output_tokens == Some(5)));
    assert!(events.iter().all(|event| event.reasoning_tokens == Some(2)));
    assert!(events.iter().all(|event| event.total_tokens == Some(16)));
    assert!(events.iter().all(|event| event.ttft_ms.is_some()));
}

#[tokio::test]
async fn websocket_upgrade_refreshes_once_on_unauthorized() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let (upstream, state) = spawn_websocket_upstream_with_behavior(
        WebSocketBehavior::UnauthorizedOnce(attempts.clone()),
    )
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    authority
        .register(
            "relay-refresh-account",
            TokenSet::new(
                "old-access",
                Some("refresh-secret".into()),
                None,
                Some(current_time_ms() + 600_000),
                current_time_ms(),
                1,
            )
            .unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let refresh = Arc::new(RefreshAdapter {
        calls: AtomicUsize::new(0),
        delay: Duration::ZERO,
        access_token: "new-access",
    });
    let (gateway, events, refresh, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "relay-refresh-account",
            "provider-refresh-account",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh,
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    assert_eq!(upgraded.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type":"response.create","model":MODEL,"input":"hello"}).to_string(),
        ))
        .await
        .unwrap();
    receive_websocket_completion(&mut socket).await;

    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
    let headers = state.headers.lock().unwrap();
    assert_eq!(headers.len(), 2);
    assert_eq!(
        header(&headers[0], AUTHORIZATION.as_str()).as_deref(),
        Some("Bearer old-access")
    );
    assert_eq!(
        header(&headers[1], AUTHORIZATION.as_str()).as_deref(),
        Some("Bearer new-access")
    );
    drop(headers);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
}

#[tokio::test]
async fn account_websocket_retries_usage_limit_before_output_and_early_close() {
    let reset_at = current_time_ms() / 1_000 + 6 * 24 * 60 * 60;
    let (status_upstream, status_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![
            json!({"type": "response.created", "response": {"id": "discarded-response"}}),
            json!({
                "type": "error",
                "status": 429,
                "body": {"error": {
                    "type": "usage_limit_reached",
                    "message": "Usage limit reached. Try again after the reset.",
                    "resets_at": reset_at
                }}
            }),
        ])))
        .await;
    let (closed_upstream, closed_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Close).await;
    let (success_upstream, success_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "status-account", "status-access").await;
    register_ready(&authority, "closed-account", "closed-access").await;
    register_ready(&authority, "success-account", "success-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("status-account", "provider-status", &status_upstream, 300),
            account("closed-account", "provider-closed", &closed_upstream, 200),
            account(
                "success-account",
                "provider-success",
                &success_upstream,
                100,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "retry me"}).to_string(),
        ))
        .await
        .unwrap();

    let first = receive_websocket_json(&mut socket).await;
    assert_eq!(first["type"], "response.output_text.delta");
    let completed = receive_websocket_completion(&mut socket).await;
    assert_eq!(completed["type"], "response.completed");
    socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "model": MODEL,
                "input": "continue after fallback",
                "previous_response_id": completed["response"]["id"]
            })
            .to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.output_text.delta"
    );
    assert_eq!(
        receive_websocket_completion(&mut socket).await["type"],
        "response.completed"
    );
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert_eq!(status_state.requests.lock().unwrap().len(), 1);
    assert_eq!(closed_state.requests.lock().unwrap().len(), 1);
    assert_eq!(success_state.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(
        events.iter().map(|event| event.attempt).collect::<Vec<_>>(),
        [1, 2, 3, 1]
    );
    assert!(events[..3]
        .iter()
        .all(|event| event.request_id == events[0].request_id));
    assert_ne!(events[0].request_id, events[3].request_id);
    assert_eq!(
        events[0].http_status,
        StatusCode::TOO_MANY_REQUESTS.as_u16()
    );
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_quota_exhausted")
    );
    assert_eq!(events[0].cooldown_scope.as_deref(), Some("*"));
    assert!(events[0].retry_at_ms.is_some());
    assert_eq!(events[0].consecutive_failures, Some(1));
    assert_eq!(
        events[1].error_category.as_deref(),
        Some("upstream_websocket_closed")
    );
    assert!(events[2].success);
    assert!(events[3].success);
}

#[tokio::test]
async fn account_websocket_does_not_retry_after_output_begins() {
    let (failing_upstream, failing_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![
            json!({"type": "response.output_text.delta", "delta": "partial"}),
            json!({
                "type": "error",
                "status": 502,
                "error": {"message": "synthetic late failure"}
            }),
        ])))
        .await;
    let (reserve_upstream, reserve_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "failing-account", "failing-access").await;
    register_ready(&authority, "reserve-account", "reserve-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account(
                "failing-account",
                "provider-failing",
                &failing_upstream,
                200,
            ),
            account(
                "reserve-account",
                "provider-reserve",
                &reserve_upstream,
                100,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "do not replay"})
                .to_string(),
        ))
        .await
        .unwrap();

    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.output_text.delta"
    );
    assert_eq!(receive_websocket_json(&mut socket).await["type"], "error");
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert_eq!(failing_state.requests.lock().unwrap().len(), 1);
    assert!(reserve_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(events[0].http_status, StatusCode::BAD_GATEWAY.as_u16());
}

#[tokio::test]
async fn account_websocket_reports_a_late_close_as_502_without_replaying() {
    let (failing_upstream, failing_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::OutputThenClose).await;
    let (reserve_upstream, reserve_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "failing-account", "failing-access").await;
    register_ready(&authority, "reserve-account", "reserve-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account(
                "failing-account",
                "provider-failing",
                &failing_upstream,
                200,
            ),
            account(
                "reserve-account",
                "provider-reserve",
                &reserve_upstream,
                100,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "close late"}).to_string(),
        ))
        .await
        .unwrap();

    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.output_text.delta"
    );
    let error = receive_websocket_json(&mut socket).await;
    assert_eq!(error["type"], "error");
    assert_eq!(error["status"], StatusCode::BAD_GATEWAY.as_u16());
    assert_eq!(error["error"]["code"], "upstream_websocket_closed");
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert_eq!(failing_state.requests.lock().unwrap().len(), 1);
    assert!(reserve_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].http_status, StatusCode::BAD_GATEWAY.as_u16());
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_websocket_closed")
    );
}

#[tokio::test]
async fn abrupt_websocket_disconnect_releases_its_lease_without_retrying() {
    let release = Arc::new(Notify::new());
    let (upstream, state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Hold(release.clone())).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "cancel me"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.output_text.delta"
    );
    assert_eq!(state.requests.lock().unwrap().len(), 1);
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        1
    );
    drop(socket);

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let idle =
                gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight == 0;
            if idle && !events.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("disconnected websocket lease was not released");
    release.notify_waiters();
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("client_websocket")
    );
    assert_eq!(events[0].retry_at_ms, None);
}

#[tokio::test]
async fn account_websocket_reselects_for_each_independent_request() {
    let (first_upstream, first_state) = spawn_websocket_upstream().await;
    let (second_upstream, second_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "first-account", "first-access").await;
    register_ready(&authority, "second-account", "second-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &first_upstream, 100),
            account("second-account", "provider-second", &second_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    for input in ["first independent request", "second independent request"] {
        socket
            .send(ClientWsMessage::Text(
                json!({"type": "response.create", "model": MODEL, "input": input}).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.output_text.delta"
        );
        assert_eq!(
            receive_websocket_completion(&mut socket).await["type"],
            "response.completed"
        );
    }
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert_eq!(first_state.requests.lock().unwrap().len(), 1);
    assert_eq!(second_state.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_ne!(events[0].candidate_id, events[1].candidate_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sse_transport_concurrency_matrix_balances_and_releases_all_leases() {
    for requests in [1, 20, 200] {
        assert_sse_concurrency(requests).await;
    }
}

async fn assert_sse_concurrency(requests: usize) {
    let (first_upstream, first_state) =
        spawn_upstream(vec![successful_sse_reply(); requests]).await;
    let (second_upstream, second_state) =
        spawn_upstream(vec![successful_sse_reply(); requests]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "first-account", "first-access").await;
    register_ready(&authority, "second-account", "second-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &first_upstream, 100),
            account("second-account", "provider-second", &second_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let client = reqwest::Client::new();
    let url = format!("{}/v1/responses", gateway.base_url);
    let completed = tokio::time::timeout(
        Duration::from_secs(20),
        join_all((0..requests).map(|index| {
            let client = client.clone();
            let url = url.clone();
            async move {
                let response = client
                    .post(url)
                    .bearer_auth(LOCAL_KEY)
                    .json(&json!({
                        "model": MODEL,
                        "input": format!("parallel SSE chat {index}"),
                        "stream": true
                    }))
                    .send()
                    .await
                    .unwrap();
                response.status() == StatusCode::OK
                    && response
                        .text()
                        .await
                        .unwrap()
                        .contains("response.completed")
            }
        })),
    )
    .await
    .expect("parallel SSE requests timed out");

    assert!(completed.into_iter().all(|completed| completed));
    assert_transport_matrix_state(
        requests,
        &first_state.requests,
        &second_state.requests,
        &events,
        &gateway,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_transport_concurrency_matrix_balances_and_releases_all_leases() {
    for requests in [1, 20, 200] {
        assert_websocket_concurrency(requests).await;
    }
}

async fn assert_websocket_concurrency(requests: usize) {
    let (first_upstream, first_state) = spawn_websocket_upstream().await;
    let (second_upstream, second_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "first-account", "first-access").await;
    register_ready(&authority, "second-account", "second-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &first_upstream, 100),
            account("second-account", "provider-second", &second_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let client = reqwest::Client::new();
    let url = format!("{}/v1/responses", gateway.base_url);
    let completed = tokio::time::timeout(
        Duration::from_secs(20),
        join_all((0..requests).map(|index| {
            let client = client.clone();
            let url = url.clone();
            async move {
                let upgraded = client
                    .get(url)
                    .bearer_auth(LOCAL_KEY)
                    .upgrade()
                    .send()
                    .await
                    .unwrap();
                assert_eq!(upgraded.status(), StatusCode::SWITCHING_PROTOCOLS);
                let mut socket = upgraded.into_websocket().await.unwrap();
                socket
                    .send(ClientWsMessage::Text(
                        json!({
                            "type": "response.create",
                            "model": MODEL,
                            "input": format!("parallel chat {index}")
                        })
                        .to_string(),
                    ))
                    .await
                    .unwrap();
                receive_websocket_completion(&mut socket).await["type"] == "response.completed"
            }
        })),
    )
    .await
    .expect("parallel websocket requests timed out");

    assert!(completed.into_iter().all(|completed| completed));
    assert_transport_matrix_state(
        requests,
        &first_state.requests,
        &second_state.requests,
        &events,
        &gateway,
    )
    .await;
}

async fn assert_transport_matrix_state<T, U>(
    requests: usize,
    first_requests: &Arc<Mutex<Vec<T>>>,
    second_requests: &Arc<Mutex<Vec<T>>>,
    events: &Arc<Mutex<Vec<U>>>,
    gateway: &TestServer,
) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while events.lock().unwrap().len() != requests {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("transport usage events did not finish");
    let first_requests = first_requests.lock().unwrap().len();
    let second_requests = second_requests.lock().unwrap().len();
    assert_eq!(first_requests + second_requests, requests);
    assert!(
        first_requests.abs_diff(second_requests) <= requests.max(20) / 20,
        "parallel routing was unexpectedly skewed: {first_requests}/{second_requests}"
    );
    assert!(gateway
        .runtime
        .as_ref()
        .unwrap()
        .candidate_runtime_order()
        .iter()
        .all(|candidate| candidate.in_flight == 0));
}

#[tokio::test]
async fn account_websocket_keeps_previous_response_on_its_current_account() {
    let (first_upstream, first_state) = spawn_websocket_upstream().await;
    let (second_upstream, second_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "first-account", "first-access").await;
    register_ready(&authority, "second-account", "second-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &first_upstream, 100),
            account("second-account", "provider-second", &second_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "start"}).to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut socket).await;
    let completed = receive_websocket_completion(&mut socket).await;
    socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "model": MODEL,
                "input": [{
                    "type": "function_call_output",
                    "call_id": "call_live",
                    "output": "done"
                }],
                "previous_response_id": completed["response"]["id"]
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut socket).await;
    let _ = receive_websocket_completion(&mut socket).await;
    tokio::time::sleep(Duration::from_millis(10)).await;

    let counts = [
        first_state.requests.lock().unwrap().len(),
        second_state.requests.lock().unwrap().len(),
    ];
    assert!(counts == [2, 0] || counts == [0, 2]);
    assert_eq!(
        first_state.headers.lock().unwrap().len() + second_state.headers.lock().unwrap().len(),
        1
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].candidate_id, events[1].candidate_id);
}

#[tokio::test]
async fn account_websocket_restores_previous_response_affinity_after_reconnect() {
    let (first_upstream, first_state) = spawn_websocket_upstream().await;
    let (second_upstream, second_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "first-account", "first-access").await;
    register_ready(&authority, "second-account", "second-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &first_upstream, 100),
            account("second-account", "provider-second", &second_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let client = reqwest::Client::new();
    let upgraded = client
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "start"}).to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut socket).await;
    let completed = receive_websocket_completion(&mut socket).await;
    let response_id = completed["response"]["id"].as_str().unwrap().to_string();
    drop(socket);

    let upgraded = client
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "model": MODEL,
                "input": "continue after reconnect",
                "previous_response_id": response_id
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut socket).await;
    let _ = receive_websocket_completion(&mut socket).await;
    tokio::time::sleep(Duration::from_millis(10)).await;

    let counts = [
        first_state.requests.lock().unwrap().len(),
        second_state.requests.lock().unwrap().len(),
    ];
    assert!(counts == [2, 0] || counts == [0, 2]);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].candidate_id, events[1].candidate_id);
}

#[tokio::test]
async fn prompt_cache_key_keeps_reconnected_websocket_on_the_same_account() {
    let (first_upstream, first_state) = spawn_websocket_upstream().await;
    let (second_upstream, second_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "first-account", "first-access").await;
    register_ready(&authority, "second-account", "second-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &first_upstream, 100),
            account("second-account", "provider-second", &second_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let client = reqwest::Client::new();
    for input in ["start", "continue"] {
        let upgraded = client
            .get(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .upgrade()
            .send()
            .await
            .unwrap();
        let mut socket = upgraded.into_websocket().await.unwrap();
        socket
            .send(ClientWsMessage::Text(
                json!({
                    "type": "response.create",
                    "model": MODEL,
                    "input": input,
                    "prompt_cache_key": "thread-1"
                })
                .to_string(),
            ))
            .await
            .unwrap();
        let _ = receive_websocket_json(&mut socket).await;
        let _ = receive_websocket_completion(&mut socket).await;
    }
    tokio::time::sleep(Duration::from_millis(10)).await;

    let counts = [
        first_state.requests.lock().unwrap().len(),
        second_state.requests.lock().unwrap().len(),
    ];
    assert!(counts == [2, 0] || counts == [0, 2]);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].candidate_id, events[1].candidate_id);
    assert_eq!(
        events[1].routing.as_ref().map(|routing| routing.reason),
        Some(SelectionReason::PromptCacheAffinity)
    );
}

#[tokio::test]
async fn unknown_websocket_response_owner_is_recovered_before_output() {
    let (wrong_upstream, wrong_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![json!({
            "type": "error",
            "status": 400,
            "error": {
                "message": "Previous response with id 'response-from-before-restart' not found.",
                "type": "invalid_request_error",
                "code": "previous_response_not_found"
            }
        })])))
        .await;
    let (owner_upstream, owner_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "wrong-account", "wrong-access").await;
    register_ready(&authority, "owner-account", "owner-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("wrong-account", "provider-wrong", &wrong_upstream, 100),
            account("owner-account", "provider-owner", &owner_upstream, 10),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "model": MODEL,
                "input": "continue after restart",
                "previous_response_id": "response-from-before-restart"
            })
            .to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.output_text.delta"
    );
    assert_eq!(
        receive_websocket_completion(&mut socket).await["type"],
        "response.completed"
    );
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert_eq!(wrong_state.requests.lock().unwrap().len(), 1);
    assert_eq!(owner_state.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("response_affinity_miss")
    );
    assert_eq!(events[0].retry_at_ms, None);
    assert_eq!(events[0].http_status, StatusCode::BAD_REQUEST.as_u16());
    assert!(events[1].success);
}

#[tokio::test]
async fn orphaned_websocket_response_resets_once_without_tool_output() {
    let (missing_upstream, missing_state) = spawn_websocket_upstream_with_behavior(
        WebSocketBehavior::Sequence(Arc::new(Mutex::new(VecDeque::from(vec![
            vec![json!({
                "type": "error",
                "status": 400,
                "error": {
                    "message": "Previous response with id 'orphaned-response' not found.",
                    "type": "invalid_request_error",
                    "code": "previous_response_not_found"
                }
            })],
            vec![
                json!({"type": "response.output_text.delta", "delta": "fresh"}),
                json!({
                    "type": "response.completed",
                    "response": {"id": "fresh-ws-response"}
                }),
            ],
        ])))),
    )
    .await;
    let (transport_upstream, transport_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![json!({
            "type": "error",
            "status": 502,
            "error": {"code": "upstream_transport", "message": "temporary transport failure"}
        })])))
        .await;
    let (limited_upstream, limited_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![json!({
            "type": "error",
            "status": 429,
            "error": {"code": "rate_limit_exceeded", "message": "rate limited"}
        })])))
        .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "missing-account", "missing-access").await;
    register_ready(&authority, "transport-account", "transport-access").await;
    register_ready(&authority, "limited-account", "limited-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account(
                "missing-account",
                "provider-missing",
                &missing_upstream,
                100,
            ),
            account(
                "transport-account",
                "provider-transport",
                &transport_upstream,
                50,
            ),
            account("limited-account", "provider-limited", &limited_upstream, 10),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "model": MODEL,
                "input": "continue without an available response owner",
                "previous_response_id": "orphaned-response"
            })
            .to_string(),
        ))
        .await
        .unwrap();

    assert_eq!(receive_websocket_json(&mut socket).await["delta"], "fresh");
    assert_eq!(
        receive_websocket_completion(&mut socket).await["response"]["id"],
        "fresh-ws-response"
    );
    let requests = missing_state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["previous_response_id"], "orphaned-response");
    assert!(requests[1].get("previous_response_id").is_none());
    assert_eq!(transport_state.requests.lock().unwrap().len(), 1);
    assert_eq!(limited_state.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("response_affinity_miss")
    );
    assert!(events[3].success);
}

#[tokio::test]
async fn stale_websocket_response_affinity_resets_before_quota_routing() {
    let (fallback_upstream, fallback_state) = spawn_websocket_upstream().await;
    let (owner_upstream, owner_state) = spawn_websocket_upstream_with_behavior(
        WebSocketBehavior::Sequence(Arc::new(Mutex::new(VecDeque::from(vec![
            vec![
                json!({"type": "response.output_text.delta", "delta": "owner"}),
                json!({"type": "response.completed", "response": {"id": "stale-ws-response"}}),
            ],
            vec![json!({
                "type": "error",
                "status": 400,
                "error": {
                    "message": "Previous response with id 'stale-ws-response' not found.",
                    "type": "invalid_request_error",
                    "code": "previous_response_not_found"
                }
            })],
            vec![
                json!({"type": "response.output_text.delta", "delta": "recovered"}),
                json!({"type": "response.completed", "response": {"id": "recovered-ws-response"}}),
            ],
        ])))),
    )
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "owner-account", "owner-access").await;
    register_ready(&authority, "fallback-account", "fallback-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("owner-account", "provider-owner", &owner_upstream, 100),
            account(
                "fallback-account",
                "provider-fallback",
                &fallback_upstream,
                10,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let client = reqwest::Client::new();
    let upgraded = client
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut first_socket = upgraded.into_websocket().await.unwrap();
    first_socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "start"}).to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut first_socket).await;
    let first_completed = receive_websocket_completion(&mut first_socket).await;
    let response_id = first_completed["response"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    drop(first_socket);
    tokio::time::sleep(Duration::from_millis(20)).await;

    let upgraded = client
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut second_socket = upgraded.into_websocket().await.unwrap();
    second_socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "model": MODEL,
                "input": "continue after stale binding",
                "previous_response_id": response_id
            })
            .to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(
        receive_websocket_json(&mut second_socket).await["delta"],
        "recovered"
    );
    assert_eq!(
        receive_websocket_completion(&mut second_socket).await["response"]["id"],
        "recovered-ws-response"
    );

    assert_eq!(owner_state.requests.lock().unwrap().len(), 3);
    assert!(fallback_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[1].error_category.as_deref(),
        Some("response_affinity_miss")
    );
    assert!(events[2].success);
}

#[tokio::test]
async fn account_non_stream_client_buffers_codex_stream_response() {
    let (upstream, state) = spawn_upstream(vec![Reply::Stream(vec![
        StreamChunk::Data(
            "data: {\"type\":\"response.in_progress\",\"response\":{\"id\":\"early-response\",\"object\":\"response\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
        ),
        StreamChunk::Data(
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"message\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n",
        ),
        StreamChunk::Data(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"account-response\",\"object\":\"response\",\"model\":\"gpt-p3\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        ),
        StreamChunk::Data("data: [DONE]\n\n"),
    ])])
    .await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["id"], "account-response");
    assert_eq!(body["output"][0]["type"], "message");
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests[0].body["store"], false);
    assert_eq!(requests[0].body["stream"], true);
    assert!(requests[0].body["input"].is_array());
    assert!(requests[0].body.get("max_output_tokens").is_none());
    drop(requests);
    let events = events.lock().unwrap();
    assert!(events[0].success);
    assert_eq!(events[0].total_tokens, Some(2));
}

#[tokio::test]
async fn high_priority_api_source_runs_before_a_healthy_oauth_account() {
    let (source_upstream, source_state) = spawn_upstream(vec![success_reply("api-first")]).await;
    let (account_upstream, account_state) =
        spawn_upstream(vec![success_reply("oauth-must-not-run")]).await;
    let authority = ready_authority("oauth-account", "oauth-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        vec![source(
            "api-source",
            &source_upstream,
            "source-key",
            1_000_000,
        )],
        vec![account(
            "oauth-account",
            "provider-account",
            &account_upstream,
            0,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Value>().await.unwrap()["id"], "api-first");
    assert_eq!(source_state.requests.lock().unwrap().len(), 1);
    assert!(account_state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn negative_priority_api_source_waits_behind_a_healthy_oauth_account() {
    let (source_upstream, source_state) =
        spawn_upstream(vec![success_reply("api-must-not-run")]).await;
    let (account_upstream, account_state) =
        spawn_upstream(vec![success_reply("oauth-first")]).await;
    let authority = ready_authority("oauth-account", "oauth-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        vec![source(
            "api-source",
            &source_upstream,
            "source-key",
            -1_000_000,
        )],
        vec![account(
            "oauth-account",
            "provider-account",
            &account_upstream,
            0,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Value>().await.unwrap()["id"], "oauth-first");
    assert_eq!(account_state.requests.lock().unwrap().len(), 1);
    assert!(source_state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn failed_account_falls_back_to_api_source_in_the_same_scheduler() {
    let (account_upstream, account_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"error": {"message": "synthetic"}}),
    )])
    .await;
    let (source_upstream, source_state) =
        spawn_upstream(vec![success_reply("source-response")]).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("source", &source_upstream, "source-key", 0)],
        vec![account(
            "relay-account",
            "provider-account",
            &account_upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "source-response"
    );
    assert_eq!(account_state.requests.lock().unwrap().len(), 1);
    assert_eq!(source_state.requests.lock().unwrap().len(), 1);
    assert_eq!(
        source_state.requests.lock().unwrap()[0]
            .authorization
            .as_deref(),
        Some("Bearer source-key")
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].account_id.as_deref(), Some("relay-account"));
    assert_eq!(events[1].account_id, None);
    assert_eq!(events[1].candidate_id.as_deref(), Some("source"));
}

#[tokio::test]
async fn terminal_account_auth_is_removed_from_following_requests() {
    let (broken_upstream, broken_state) = spawn_upstream(Vec::new()).await;
    let (ready_upstream, ready_state) = spawn_upstream(vec![
        success_reply("ready-first"),
        success_reply("ready-second"),
    ])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    authority
        .register(
            "oauth-broken",
            TokenSet::access_only("expired-access", Some(1), 0).unwrap(),
            AccountAuthState::RequiresReauth(ReauthReason::InvalidatedRefreshToken),
        )
        .await
        .unwrap();
    register_ready(&authority, "oauth-ready", "ready-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("oauth-broken", "provider-broken", &broken_upstream, 9_000),
            account("oauth-ready", "provider-ready", &ready_upstream, 8_000),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert!(broken_state.requests.lock().unwrap().is_empty());
    assert_eq!(ready_state.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.account_id.as_deref() == Some("oauth-broken"))
            .count(),
        1
    );
    assert_eq!(events[0].error_category.as_deref(), Some("account_auth"));
    assert!(
        !gateway
            .runtime
            .as_ref()
            .unwrap()
            .candidate_runtime_order()
            .iter()
            .find(|candidate| candidate.candidate_id == "oauth-broken")
            .unwrap()
            .available
    );
}

#[tokio::test]
async fn retryable_oauth_failure_prefers_next_oauth_before_paid_source() {
    let (primary_upstream, primary_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"error": {"message": "synthetic"}}),
    )])
    .await;
    let (secondary_upstream, secondary_state) =
        spawn_upstream(vec![success_reply("secondary-account")]).await;
    let (paid_upstream, paid_state) =
        spawn_upstream(vec![success_reply("paid-source-must-not-run")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "oauth-primary", "primary-access").await;
    register_ready(&authority, "oauth-secondary", "secondary-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("paid-source", &paid_upstream, "paid-key", 100)],
        vec![
            account("oauth-primary", "provider-primary", &primary_upstream, 300),
            account(
                "oauth-secondary",
                "provider-secondary",
                &secondary_upstream,
                200,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "secondary-account"
    );
    assert_eq!(primary_state.requests.lock().unwrap().len(), 1);
    assert_eq!(secondary_state.requests.lock().unwrap().len(), 1);
    assert!(paid_state.requests.lock().unwrap().is_empty());
    assert_eq!(
        secondary_state.requests.lock().unwrap()[0]
            .authorization
            .as_deref(),
        Some("Bearer secondary-access")
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].candidate_id.as_deref(), Some("oauth-primary"));
    assert_eq!(events[1].candidate_id.as_deref(), Some("oauth-secondary"));
}

#[tokio::test]
async fn exhausted_and_reauth_accounts_are_filtered_before_account_source_fallback() {
    let (exhausted_upstream, exhausted_state) =
        spawn_upstream(vec![success_reply("exhausted-must-not-run")]).await;
    let (reauth_upstream, reauth_state) =
        spawn_upstream(vec![success_reply("reauth-must-not-run")]).await;
    let (eligible_upstream, eligible_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"error": {"message": "synthetic"}}),
    )])
    .await;
    let (source_upstream, source_state) =
        spawn_upstream(vec![success_reply("source-fallback")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "oauth-exhausted", "exhausted-access").await;
    register_ready(&authority, "oauth-reauth", "reauth-access").await;
    register_ready(&authority, "oauth-eligible", "eligible-access").await;
    let mut exhausted = account(
        "oauth-exhausted",
        "provider-exhausted",
        &exhausted_upstream,
        400,
    );
    exhausted.quota = CandidateQuota::Exhausted;
    let mut reauth = account("oauth-reauth", "provider-reauth", &reauth_upstream, 300);
    reauth.health = CandidateHealth::ReauthRequired;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source(
            "source-fallback",
            &source_upstream,
            "source-key",
            100,
        )],
        vec![
            exhausted,
            reauth,
            account(
                "oauth-eligible",
                "provider-eligible",
                &eligible_upstream,
                200,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "source-fallback"
    );
    assert!(exhausted_state.requests.lock().unwrap().is_empty());
    assert!(reauth_state.requests.lock().unwrap().is_empty());
    assert_eq!(eligible_state.requests.lock().unwrap().len(), 1);
    assert_eq!(source_state.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].candidate_id.as_deref(), Some("oauth-eligible"));
    assert_eq!(events[1].candidate_id.as_deref(), Some("source-fallback"));
}

#[tokio::test]
async fn exhausted_account_keeps_codex_catalog_but_does_not_receive_requests() {
    let (upstream, state) = spawn_upstream(vec![success_reply("must-not-run")]).await;
    let authority = ready_authority("oauth-exhausted", "exhausted-access").await;
    let mut exhausted = account("oauth-exhausted", "provider-exhausted", &upstream, 100);
    exhausted.quota = CandidateQuota::Exhausted;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![exhausted],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let catalog: Value = reqwest::Client::new()
        .get(format!(
            "{}/v1/models?client_version={CODEX_MODELS_CLIENT_VERSION}",
            gateway.base_url,
        ))
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(catalog["models"][0]["slug"], MODEL);
    assert_eq!(catalog["models"][0]["service_tiers"][0]["id"], "priority");

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"]["code"],
        "no_eligible_source"
    );
    assert_eq!(state.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn persisted_account_cooldown_and_failure_count_are_ignored() {
    let (upstream, state) = spawn_upstream(vec![success_reply("account-ready")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "oauth-account", "account-access").await;
    let mut account = account("oauth-account", "provider-account", &upstream, 300);
    account
        .cooldowns
        .insert(MODEL.to_string(), current_time_ms().saturating_add(60_000));
    account.consecutive_failures = 7;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "account-ready"
    );
    assert_eq!(state.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].consecutive_failures, Some(0));
}

#[tokio::test]
async fn http_usage_limit_immediately_excludes_the_account_until_quota_refresh() {
    let reset_at = current_time_ms() / 1_000 + 60 * 60;
    let (limited_upstream, limited_state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error": {
                "type": "usage_limit_reached",
                "message": "Usage limit reached",
                "resets_at": reset_at
            }}),
        ),
        success_reply("recovered"),
    ])
    .await;
    let (fallback_upstream, fallback_state) = spawn_upstream(vec![
        success_reply("fallback-1"),
        success_reply("fallback-2"),
    ])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "limited-account", "limited-access").await;
    register_ready(&authority, "fallback-account", "fallback-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account(
                "limited-account",
                "provider-limited",
                &limited_upstream,
                200,
            ),
            account(
                "fallback-account",
                "provider-fallback",
                &fallback_upstream,
                100,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let first = request(&gateway, false).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.json::<Value>().await.unwrap()["id"], "fallback-1");
    let second = request(&gateway, false).await;
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(second.json::<Value>().await.unwrap()["id"], "fallback-2");
    assert_eq!(limited_state.requests.lock().unwrap().len(), 1);
    assert_eq!(fallback_state.requests.lock().unwrap().len(), 2);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].candidate_id.as_deref(), Some("limited-account"));
    assert_eq!(events[0].cooldown_scope.as_deref(), Some("*"));
    assert!(events[0].retry_at_ms.is_some());
    assert_eq!(events[0].consecutive_failures, Some(1));
    assert_eq!(events[2].candidate_id.as_deref(), Some("fallback-account"));
    assert!(events[2].success);
    assert_eq!(events[2].cooldown_scope, None);
    assert_eq!(events[2].retry_at_ms, None);
}

#[tokio::test]
async fn retry_cap_stops_account_rotation_after_429() {
    let (limited_upstream, limited_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error": {"code": "rate_limit_exceeded"}}),
    )])
    .await;
    let (ready_upstream, ready_state) =
        spawn_upstream(vec![success_reply("rotated-response")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "limited-account", "limited-access").await;
    register_ready(&authority, "ready-account", "ready-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![
            account(
                "limited-account",
                "provider-limited",
                &limited_upstream,
                200,
            ),
            account("ready-account", "provider-ready", &ready_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions {
            max_retry_candidates: 1,
            ..GatewayRuntimeOptions::default()
        },
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited_state.requests.lock().unwrap().len(), 1);
    assert!(ready_state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn retry_cap_stops_compact_account_rotation_after_429() {
    let (limited_upstream, limited_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error": {"code": "rate_limit_exceeded"}}),
    )])
    .await;
    let (ready_upstream, ready_state) =
        spawn_upstream(vec![success_reply("rotated-response")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "limited-account", "limited-access").await;
    register_ready(&authority, "ready-account", "ready-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![
            account(
                "limited-account",
                "provider-limited",
                &limited_upstream,
                200,
            ),
            account("ready-account", "provider-ready", &ready_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions {
            max_retry_candidates: 1,
            ..GatewayRuntimeOptions::default()
        },
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "compact this",
            "max_output_tokens": 16,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited_state.requests.lock().unwrap().len(), 1);
    assert!(ready_state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn local_key_account_scope_uses_relay_ids_not_provider_header_ids() {
    let (allowed_upstream, allowed_state) = spawn_upstream(vec![success_reply("allowed")]).await;
    let (denied_upstream, denied_state) = spawn_upstream(vec![success_reply("denied")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "allowed-local", "allowed-access").await;
    register_ready(&authority, "denied-local", "denied-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("denied-local", "provider-denied", &denied_upstream, 100),
            account("allowed-local", "provider-allowed", &allowed_upstream, 0),
        ],
        vec![mixed_key(None, Some(vec!["allowed-local"]))],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    assert_eq!(models(&gateway).await, [MODEL]);
    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(allowed_state.requests.lock().unwrap().len(), 1);
    assert!(denied_state.requests.lock().unwrap().is_empty());
    assert_eq!(
        allowed_state.requests.lock().unwrap()[0]
            .chatgpt_account_id
            .as_deref(),
        Some("provider-allowed")
    );
}

#[tokio::test]
async fn concurrent_gateway_requests_rotate_and_persist_one_token_once() {
    let (upstream, state) = spawn_upstream(Vec::new()).await;
    let authority = Arc::new(TokenAuthority::new(2).unwrap());
    authority
        .register(
            "relay-refresh-account",
            TokenSet::new(
                "expired-access",
                Some("refresh-secret".into()),
                None,
                Some(1),
                0,
                0,
            )
            .unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let refresh = Arc::new(RefreshAdapter {
        calls: AtomicUsize::new(0),
        delay: Duration::from_millis(25),
        access_token: "rotated-access",
    });
    let persistence = Arc::new(PersistenceAdapter::default());
    let (gateway, _, refresh, persistence) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "relay-refresh-account",
            "provider-refresh-account",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh,
        persistence,
    )
    .await;

    let client = reqwest::Client::new();
    let responses = join_all((0..20).map(|_| {
        let client = client.clone();
        let url = format!("{}/v1/responses", gateway.base_url);
        async move {
            client
                .post(url)
                .bearer_auth(LOCAL_KEY)
                .json(&json!({"model": MODEL, "input": "hello"}))
                .send()
                .await
                .unwrap()
        }
    }))
    .await;
    assert!(responses
        .iter()
        .all(|response| response.status() == StatusCode::OK));
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
    assert_eq!(persistence.token_writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        *persistence.persisted_accounts.lock().unwrap(),
        vec!["relay-refresh-account".to_string()]
    );
    assert_eq!(
        *persistence.auth_states.lock().unwrap(),
        vec![(
            "relay-refresh-account".to_string(),
            AccountAuthState::Active
        )]
    );
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 20);
    assert!(requests
        .iter()
        .all(|request| request.authorization.as_deref() == Some("Bearer rotated-access")));
}

#[tokio::test]
async fn concurrent_new_chats_are_balanced_across_equal_accounts() {
    const REQUESTS: usize = 200;
    let (first_upstream, first_state) = spawn_delayed_upstream(Duration::from_millis(100)).await;
    let (second_upstream, second_state) = spawn_delayed_upstream(Duration::from_millis(100)).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "account-first", "first-access").await;
    register_ready(&authority, "account-second", "second-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("account-first", "provider-first", &first_upstream, 10),
            account("account-second", "provider-second", &second_upstream, 10),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let responses = join_all((0..REQUESTS).map(|_| request(&gateway, false))).await;

    assert!(responses
        .iter()
        .all(|response| response.status() == StatusCode::OK));
    assert_eq!(first_state.requests.lock().unwrap().len(), REQUESTS / 2);
    assert_eq!(second_state.requests.lock().unwrap().len(), REQUESTS / 2);
    assert!(gateway
        .runtime
        .as_ref()
        .unwrap()
        .candidate_runtime_order()
        .iter()
        .all(|candidate| candidate.in_flight == 0));
}

#[tokio::test]
async fn independent_chat_shares_the_highest_quota_account_while_an_sse_stream_is_open() {
    let (stream_upstream, stream_state) = spawn_held_stream_upstream().await;
    let (free_upstream, free_state) = spawn_upstream(vec![success_reply("free-response")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "stream-account", "stream-access").await;
    register_ready(&authority, "free-account", "free-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("stream-account", "provider-stream", &stream_upstream, 10),
            account("free-account", "provider-free", &free_upstream, 0),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let open_stream = request(&gateway, true).await;
    assert_eq!(open_stream.status(), StatusCode::OK);
    assert_eq!(stream_state.requests.lock().unwrap().len(), 1);

    let independent = request(&gateway, false);
    let release = async {
        tokio::time::timeout(Duration::from_secs(2), async {
            while stream_state.requests.lock().unwrap().len() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the second chat did not reach the highest-quota account");
        stream_state.release.notify_waiters();
    };
    let (independent, ()) = tokio::join!(independent, release);
    assert_eq!(independent.status(), StatusCode::OK);
    assert_eq!(stream_state.requests.lock().unwrap().len(), 2);
    assert!(free_state.requests.lock().unwrap().is_empty());

    stream_state.release.notify_one();
    let _ = open_stream.bytes().await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events
        .iter()
        .all(|event| event.account_id.as_deref() == Some("stream-account")));
}

#[tokio::test]
async fn oauth_accounts_use_independent_upstream_connection_pools() {
    let (upstream, state) = spawn_connection_affinity_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "first-account", "first-access").await;
    register_ready(&authority, "second-account", "second-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &upstream, 100),
            account("second-account", "provider-second", &upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions {
            max_retry_candidates: 1,
            ..GatewayRuntimeOptions::default()
        },
    )
    .await;

    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);

    let account_ids = state.account_ids.lock().unwrap();
    assert_eq!(account_ids.len(), 2);
    assert_ne!(account_ids[0], account_ids[1]);
    assert_eq!(state.owners.lock().unwrap().len(), 2);
    assert_eq!(events.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn cancelled_sse_releases_its_lease_without_cooling_the_account() {
    let (upstream, state) = spawn_held_stream_upstream().await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        1
    );
    drop(response);

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let idle =
                gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight == 0;
            if idle && !events.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cancelled SSE lease was not released");
    state.release.notify_waiters();
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("client_cancelled")
    );
    assert_eq!(events[0].retry_at_ms, None);
}

#[tokio::test]
async fn account_stream_never_falls_back_after_first_event() {
    let (account_upstream, account_state) = spawn_upstream(vec![Reply::Stream(vec![
        StreamChunk::Data("data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n"),
        StreamChunk::Error,
    ])])
    .await;
    let (source_upstream, source_state) = spawn_upstream(vec![success_reply("must-not-run")]).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("source", &source_upstream, "source-key", 0)],
        vec![account(
            "relay-account",
            "provider-account",
            &account_upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );
    let _ = response.bytes().await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(account_state.requests.lock().unwrap().len(), 1);
    assert!(source_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].account_id.as_deref(), Some("relay-account"));
    assert!(!events[0].success);
}

#[tokio::test]
async fn agent_identity_account_signs_each_gateway_request() {
    let (upstream, state) = spawn_upstream(vec![success_reply("ok")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    let mut agent_identities = HashMap::new();
    agent_identities.insert(
        "relay-agent".to_string(),
        AgentIdentityCredential::new(
            "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g".into(),
            "runtime-test".into(),
            "task-test".into(),
        )
        .unwrap(),
    );
    let (gateway, _, _, _) = spawn_mixed_gateway_with_agent_identities(
        Vec::new(),
        vec![account("relay-agent", "provider-agent", &upstream, 10_000)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        agent_identities,
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0]
        .authorization
        .as_deref()
        .is_some_and(|value| value.starts_with("AgentAssertion ")));
}

fn account(
    id: &str,
    chatgpt_account_id: &str,
    server: &TestServer,
    quota_remaining_basis_points: i32,
) -> RuntimeChatGptAccount {
    RuntimeChatGptAccount {
        id: id.to_string(),
        source_id: "openai-codex".to_string(),
        chatgpt_account_id: chatgpt_account_id.to_string(),
        responses_url: format!("{}/v1/responses", server.base_url),
        models: vec![MODEL.to_string()],
        enabled: true,
        draining: false,
        priority: 0,
        weight: 1,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        health: CandidateHealth::Healthy,
        quota: u64::try_from(quota_remaining_basis_points)
            .ok()
            .filter(|remaining| *remaining > 0)
            .map_or(CandidateQuota::Unknown, CandidateQuota::Available),
        quota_updated_at_ms: None,
        quota_snapshot: Default::default(),
        subscription_plan_type: None,
        subscription_expires_at_ms: None,
        last_used_at_ms: None,
        cooldowns: Default::default(),
        consecutive_failures: 0,
        proxy: None,
    }
}

fn source(id: &str, server: &TestServer, key: &str, priority: i32) -> RuntimeSource {
    RuntimeSource {
        source: ProviderSource {
            id: id.to_string(),
            name: id.to_string(),
            base_url: format!("{}/v1", server.base_url),
            api_key: key.to_string(),
            wire_api: WireApi::Responses,
            models: vec![MODEL.to_string()],
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

fn mixed_key(
    source_ids: Option<Vec<&str>>,
    account_ids: Option<Vec<&str>>,
) -> RuntimeMixedLocalKey {
    RuntimeMixedLocalKey {
        key: LocalGatewayKey {
            id: "local-key".to_string(),
            secret: LOCAL_KEY.to_string(),
        },
        enabled: true,
        source_ids: source_ids.map(|ids| ids.into_iter().map(str::to_string).collect()),
        account_ids: account_ids.map(|ids| ids.into_iter().map(str::to_string).collect()),
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        model_prefix: None,
        wire_apis: None,
    }
}

fn refresh_adapter() -> Arc<RefreshAdapter> {
    Arc::new(RefreshAdapter {
        calls: AtomicUsize::new(0),
        delay: Duration::ZERO,
        access_token: "unused-access",
    })
}

async fn ready_authority(account_id: &str, access_token: &str) -> Arc<TokenAuthority> {
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, account_id, access_token).await;
    authority
}

async fn register_ready(authority: &TokenAuthority, account_id: &str, access_token: &str) {
    authority
        .register(
            account_id,
            TokenSet::access_only(access_token, None, 0).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
}

async fn spawn_mixed_gateway(
    sources: Vec<RuntimeSource>,
    accounts: Vec<RuntimeChatGptAccount>,
    keys: Vec<RuntimeMixedLocalKey>,
    authority: Arc<TokenAuthority>,
    refresh: Arc<RefreshAdapter>,
    persistence: Arc<PersistenceAdapter>,
) -> (
    TestServer,
    Arc<Mutex<Vec<UsageEvent>>>,
    Arc<RefreshAdapter>,
    Arc<PersistenceAdapter>,
) {
    spawn_mixed_gateway_with_options(
        sources,
        accounts,
        keys,
        authority,
        refresh,
        persistence,
        GatewayRuntimeOptions::default(),
    )
    .await
}

async fn spawn_mixed_gateway_with_options(
    sources: Vec<RuntimeSource>,
    accounts: Vec<RuntimeChatGptAccount>,
    keys: Vec<RuntimeMixedLocalKey>,
    authority: Arc<TokenAuthority>,
    refresh: Arc<RefreshAdapter>,
    persistence: Arc<PersistenceAdapter>,
    options: GatewayRuntimeOptions,
) -> (
    TestServer,
    Arc<Mutex<Vec<UsageEvent>>>,
    Arc<RefreshAdapter>,
    Arc<PersistenceAdapter>,
) {
    spawn_mixed_gateway_with_agent_identities_and_options(
        sources,
        accounts,
        keys,
        authority,
        refresh,
        persistence,
        options,
        HashMap::new(),
    )
    .await
}

async fn spawn_mixed_gateway_with_agent_identities(
    sources: Vec<RuntimeSource>,
    accounts: Vec<RuntimeChatGptAccount>,
    keys: Vec<RuntimeMixedLocalKey>,
    authority: Arc<TokenAuthority>,
    refresh: Arc<RefreshAdapter>,
    persistence: Arc<PersistenceAdapter>,
    agent_identities: HashMap<String, AgentIdentityCredential>,
) -> (
    TestServer,
    Arc<Mutex<Vec<UsageEvent>>>,
    Arc<RefreshAdapter>,
    Arc<PersistenceAdapter>,
) {
    spawn_mixed_gateway_with_agent_identities_and_options(
        sources,
        accounts,
        keys,
        authority,
        refresh,
        persistence,
        GatewayRuntimeOptions::default(),
        agent_identities,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn spawn_mixed_gateway_with_agent_identities_and_options(
    sources: Vec<RuntimeSource>,
    accounts: Vec<RuntimeChatGptAccount>,
    keys: Vec<RuntimeMixedLocalKey>,
    authority: Arc<TokenAuthority>,
    refresh: Arc<RefreshAdapter>,
    persistence: Arc<PersistenceAdapter>,
    options: GatewayRuntimeOptions,
    agent_identities: HashMap<String, AgentIdentityCredential>,
) -> (
    TestServer,
    Arc<Mutex<Vec<UsageEvent>>>,
    Arc<RefreshAdapter>,
    Arc<PersistenceAdapter>,
) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let runtime = Arc::new(
        GatewayRuntime::from_mixed_pool(
            sources,
            accounts,
            keys,
            RuntimeChatGptAuth {
                token_authority: authority,
                refresh_adapter: refresh.clone(),
                persistence_adapter: persistence.clone(),
                refresh_skew_ms: 0,
                agent_identities,
            },
            options,
            Arc::new(move |event| captured.lock().unwrap().push(event)),
        )
        .unwrap(),
    );
    let mut server = spawn(gateway::router(runtime.clone())).await;
    server.runtime = Some(runtime);
    (server, events, refresh, persistence)
}

async fn spawn_upstream(replies: Vec<Reply>) -> (TestServer, UpstreamState) {
    spawn_upstream_with_catalog(replies, default_upstream_model_catalog()).await
}

async fn spawn_upstream_with_catalog(
    replies: Vec<Reply>,
    model_catalog: Value,
) -> (TestServer, UpstreamState) {
    let state = UpstreamState {
        replies: Arc::new(Mutex::new(replies.into())),
        requests: Arc::new(Mutex::new(Vec::new())),
        delay: Duration::ZERO,
        model_catalog,
    };
    let app = Router::new()
        .route("/v1/models", get(upstream_models))
        .route("/v1/responses", post(upstream))
        .route("/v1/responses/compact", post(upstream))
        .route("/v1/alpha/search", post(upstream))
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn_websocket_upstream() -> (TestServer, WebSocketUpstreamState) {
    spawn_websocket_upstream_with_behavior(WebSocketBehavior::Success).await
}

async fn spawn_websocket_upstream_with_behavior(
    behavior: WebSocketBehavior,
) -> (TestServer, WebSocketUpstreamState) {
    let state = WebSocketUpstreamState {
        behavior,
        ..WebSocketUpstreamState::default()
    };
    let app = Router::new()
        .route("/v1/responses", get(upstream_websocket))
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn_delayed_upstream(delay: Duration) -> (TestServer, UpstreamState) {
    let state = UpstreamState {
        delay,
        model_catalog: default_upstream_model_catalog(),
        ..UpstreamState::default()
    };
    let app = Router::new()
        .route("/v1/models", get(upstream_models))
        .route("/v1/responses", post(upstream))
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn_held_stream_upstream() -> (TestServer, HeldStreamState) {
    let state = HeldStreamState::default();
    let app = Router::new()
        .route("/v1/responses", post(held_stream_upstream))
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn spawn_connection_affinity_upstream() -> (TestServer, ConnectionAffinityState) {
    let state = ConnectionAffinityState::default();
    let app = Router::new()
        .route("/v1/responses", post(connection_affinity_upstream))
        .with_state(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    (
        TestServer {
            base_url: format!("http://{address}"),
            task,
            runtime: None,
        },
        state,
    )
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
        runtime: None,
    }
}

async fn upstream(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response<Body> {
    state.requests.lock().unwrap().push(ObservedRequest {
        path: uri.path().to_string(),
        authorization: header(&headers, AUTHORIZATION.as_str()),
        chatgpt_account_id: header(&headers, "chatgpt-account-id"),
        originator: header(&headers, "originator"),
        responses_lite: header(&headers, "x-openai-internal-codex-responses-lite"),
        session_id: header(&headers, "x-session-id"),
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    });
    tokio::time::sleep(state.delay).await;
    match state
        .replies
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or_else(|| success_reply("default-response"))
    {
        Reply::Json(status, body) => Response::builder()
            .status(status)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        Reply::JsonWithHeaders(status, body, headers) => {
            let mut response = Response::builder()
                .status(status)
                .header(CONTENT_TYPE, "application/json");
            for (name, value) in headers {
                response = response.header(name, value);
            }
            response.body(Body::from(body.to_string())).unwrap()
        }
        Reply::Stream(chunks) => {
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
                .body(Body::from_stream(chunks))
                .unwrap()
        }
    }
}

async fn held_stream_upstream(
    State(state): State<HeldStreamState>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response<Body> {
    state.requests.lock().unwrap().push(ObservedRequest {
        path: uri.path().to_string(),
        authorization: header(&headers, AUTHORIZATION.as_str()),
        chatgpt_account_id: header(&headers, "chatgpt-account-id"),
        originator: header(&headers, "originator"),
        responses_lite: header(&headers, "x-openai-internal-codex-responses-lite"),
        session_id: header(&headers, "x-session-id"),
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    });
    let release = state.release.clone();
    let chunks = stream::unfold(0_u8, move |step| {
        let release = release.clone();
        async move {
            let chunk = match step {
                0 => Bytes::from_static(
                    b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"first\"}\n\n",
                ),
                1 => {
                    release.notified().await;
                    Bytes::from_static(b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"held-response\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n")
                }
                2 => Bytes::from_static(b"data: [DONE]\n\n"),
                _ => return None,
            };
            Some((Ok::<_, io::Error>(chunk), step + 1))
        }
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from_stream(chunks))
        .unwrap()
}

async fn connection_affinity_upstream(
    State(state): State<ConnectionAffinityState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response<Body> {
    let account_id = header(&headers, "chatgpt-account-id").unwrap_or_default();
    state.account_ids.lock().unwrap().push(account_id.clone());
    let conflict = {
        let mut owners = state.owners.lock().unwrap();
        match owners.entry(peer) {
            std::collections::hash_map::Entry::Occupied(owner) => owner.get() != &account_id,
            std::collections::hash_map::Entry::Vacant(owner) => {
                owner.insert(account_id);
                false
            }
        }
    };
    let (status, body) = if conflict {
        (
            StatusCode::BAD_GATEWAY,
            json!({"error": {"code": "connection_identity_conflict"}}),
        )
    } else {
        (
            StatusCode::OK,
            json!({
                "id": "isolated-response",
                "object": "response",
                "model": MODEL,
                "output": [],
                "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
            }),
        )
    };
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn upstream_websocket(
    State(state): State<WebSocketUpstreamState>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response<Body> {
    state.headers.lock().unwrap().push(headers);
    if let WebSocketBehavior::UnauthorizedOnce(attempts) = &state.behavior {
        if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            return Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({"error":{"code":"token_expired"}}).to_string(),
                ))
                .unwrap();
        }
    }
    websocket.on_upgrade(move |mut socket| async move {
        while let Some(Ok(message)) = socket.recv().await {
            let request = match message {
                AxumWsMessage::Text(text) => serde_json::from_slice(text.as_bytes()),
                AxumWsMessage::Binary(bytes) => serde_json::from_slice(&bytes),
                AxumWsMessage::Close(_) => break,
                AxumWsMessage::Ping(payload) => {
                    if socket.send(AxumWsMessage::Pong(payload)).await.is_err() {
                        break;
                    }
                    continue;
                }
                AxumWsMessage::Pong(_) => continue,
            };
            let Ok(request) = request else {
                break;
            };
            state.requests.lock().unwrap().push(request);
            let events = match &state.behavior {
                WebSocketBehavior::Success | WebSocketBehavior::UnauthorizedOnce(_) => vec![
                    json!({"type": "response.output_text.delta", "delta": "hello"}),
                    json!({
                        "type": "response.completed",
                        "response": {
                            "id": "ws-response",
                            "usage": {
                                "input_tokens": 11,
                                "input_tokens_details": {"cached_tokens": 7},
                                "output_tokens": 5,
                                "output_tokens_details": {"reasoning_tokens": 2},
                                "total_tokens": 16
                            }
                        }
                    }),
                ],
                WebSocketBehavior::Events(events) => events.as_ref().clone(),
                WebSocketBehavior::Sequence(events) => {
                    events.lock().unwrap().pop_front().unwrap_or_default()
                }
                WebSocketBehavior::Hold(release) => {
                    if socket
                        .send(AxumWsMessage::Text(
                            json!({"type": "response.output_text.delta", "delta": "held"})
                                .to_string()
                                .into(),
                        ))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    release.notified().await;
                    vec![json!({
                        "type": "response.completed",
                        "response": {
                            "id": "ws-held-response",
                            "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
                        }
                    })]
                }
                WebSocketBehavior::Close => {
                    let _ = socket.send(AxumWsMessage::Close(None)).await;
                    return;
                }
                WebSocketBehavior::OutputThenClose => {
                    vec![json!({"type": "response.output_text.delta", "delta": "partial"})]
                }
            };
            for event in events {
                tokio::time::sleep(Duration::from_millis(2)).await;
                if socket
                    .send(AxumWsMessage::Text(event.to_string().into()))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            if matches!(&state.behavior, WebSocketBehavior::OutputThenClose) {
                let _ = socket.send(AxumWsMessage::Close(None)).await;
                return;
            }
        }
    })
}

async fn receive_websocket_json(socket: &mut reqwest_websocket::WebSocket) -> Value {
    loop {
        let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let ClientWsMessage::Text(text) = message {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

async fn receive_websocket_completion(socket: &mut reqwest_websocket::WebSocket) -> Value {
    loop {
        let value = receive_websocket_json(socket).await;
        if value["type"] == "response.completed" {
            return value;
        }
    }
}

async fn upstream_models(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    uri: axum::http::Uri,
) -> (StatusCode, Json<Value>) {
    let client_version = uri
        .query()
        .and_then(|query| query.strip_prefix("client_version="))
        .unwrap_or_default();
    state.requests.lock().unwrap().push(ObservedRequest {
        path: uri.path().to_string(),
        authorization: header(&headers, AUTHORIZATION.as_str()),
        chatgpt_account_id: header(&headers, "chatgpt-account-id"),
        originator: header(&headers, "originator"),
        responses_lite: header(&headers, "x-openai-internal-codex-responses-lite"),
        session_id: header(&headers, "x-session-id"),
        body: json!({ "client_version": client_version }),
    });
    let status = if client_version == CODEX_MODELS_CLIENT_VERSION {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    (status, Json(state.model_catalog.clone()))
}

fn default_upstream_model_catalog() -> Value {
    json!({
        "models": [{
            "slug": MODEL,
            "display_name": "GPT P3",
            "visibility": "list",
            "supported_in_api": true,
            "service_tiers": [{
                "id": "priority",
                "name": "Fast",
                "description": "Synthetic fast tier"
            }],
            "additional_speed_tiers": ["fast"],
            "default_service_tier": "priority",
            "use_responses_lite": true,
            "supports_parallel_tool_calls": true,
            "default_reasoning_level": "high",
            "supported_reasoning_levels": [
                {"effort": "low", "description": "Low"},
                {"effort": "high", "description": "High"},
                {"effort": "xhigh", "description": "Extra high"}
            ],
            "supports_reasoning_summary_parameter": true,
            "supports_reasoning_summaries": true,
            "default_reasoning_summary": "detailed"
        }]
    })
}

fn header(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

fn success_reply(id: &str) -> Reply {
    Reply::Json(
        StatusCode::OK,
        json!({
            "id": id,
            "object": "response",
            "model": MODEL,
            "output": [],
            "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
        }),
    )
}

fn successful_sse_reply() -> Reply {
    Reply::Stream(vec![
        StreamChunk::Data(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
        ),
        StreamChunk::Data(
            "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        ),
        StreamChunk::Data("data: [DONE]\n\n"),
    ])
}

async fn request(gateway: &TestServer, stream: bool) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "hello",
            "stream": stream,
            "max_output_tokens": 16
        }))
        .send()
        .await
        .unwrap()
}

async fn models(gateway: &TestServer) -> Vec<String> {
    let body: Value = reqwest::Client::new()
        .get(format!("{}/v1/models", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
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

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

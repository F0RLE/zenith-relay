use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, Response, StatusCode};
use axum::routing::post;
use axum::Router;
use futures_util::future::{join_all, BoxFuture};
use futures_util::stream;
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use zenith_relay_core::accounts::{
    AccountAuthState, TokenAuthority, TokenPersistenceAdapter, TokenPersistenceFailure,
    TokenRefresh, TokenRefreshAdapter, TokenRefreshFailure, TokenSet,
};
use zenith_relay_core::gateway;
use zenith_relay_core::{
    CandidateHealth, CandidateQuota, GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey,
    ProviderSource, RuntimeAccount, RuntimeAccountAuth, RuntimeMixedLocalKey, RuntimeSource,
    UsageEvent, WireApi,
};

const LOCAL_KEY: &str = "p3-local-key";
const MODEL: &str = "gpt-p3";

#[derive(Clone, Debug)]
struct ObservedRequest {
    authorization: Option<String>,
    chatgpt_account_id: Option<String>,
    originator: Option<String>,
    body: Value,
}

#[derive(Clone)]
enum Reply {
    Json(StatusCode, Value),
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
async fn persisted_account_cooldown_and_failure_count_seed_the_rebuilt_runtime() {
    let (cooled_upstream, cooled_state) =
        spawn_upstream(vec![success_reply("cooled-must-not-run")]).await;
    let (limited_upstream, limited_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error": {"message": "synthetic"}}),
    )])
    .await;
    let (source_upstream, source_state) =
        spawn_upstream(vec![success_reply("restart-fallback")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "oauth-cooled", "cooled-access").await;
    register_ready(&authority, "oauth-limited", "limited-access").await;
    let before_ms = current_time_ms();
    let mut cooled = account("oauth-cooled", "provider-cooled", &cooled_upstream, 300);
    cooled
        .cooldowns
        .insert(MODEL.to_string(), before_ms.saturating_add(60_000));
    cooled.consecutive_failures = 7;
    let mut limited = account("oauth-limited", "provider-limited", &limited_upstream, 200);
    limited.consecutive_failures = 3;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source(
            "source-fallback",
            &source_upstream,
            "source-key",
            100,
        )],
        vec![cooled, limited],
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
        "restart-fallback"
    );
    assert!(cooled_state.requests.lock().unwrap().is_empty());
    assert_eq!(limited_state.requests.lock().unwrap().len(), 1);
    assert_eq!(source_state.requests.lock().unwrap().len(), 1);
    let after_ms = current_time_ms();
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].candidate_id.as_deref(), Some("oauth-limited"));
    assert_eq!(events[0].cooldown_scope.as_deref(), Some(MODEL));
    assert_eq!(events[0].consecutive_failures, Some(4));
    assert!(events[0].retry_at_ms.is_some_and(|retry_at_ms| {
        (before_ms.saturating_add(7_000)..=after_ms.saturating_add(9_000)).contains(&retry_at_ms)
    }));
    assert_eq!(events[1].consecutive_failures, Some(0));
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

fn account(
    id: &str,
    chatgpt_account_id: &str,
    server: &TestServer,
    priority: i32,
) -> RuntimeAccount {
    RuntimeAccount {
        id: id.to_string(),
        source_id: "openai-codex".to_string(),
        chatgpt_account_id: chatgpt_account_id.to_string(),
        responses_url: format!("{}/v1/responses", server.base_url),
        models: vec![MODEL.to_string()],
        enabled: true,
        draining: false,
        priority,
        weight: 1,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        health: CandidateHealth::Healthy,
        quota: CandidateQuota::Unknown,
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
        enabled: true,
        draining: false,
        priority,
        weight: 1,
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
    accounts: Vec<RuntimeAccount>,
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
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let runtime = GatewayRuntime::from_mixed_pool(
        sources,
        accounts,
        keys,
        RuntimeAccountAuth {
            token_authority: authority,
            refresh_adapter: refresh.clone(),
            persistence_adapter: persistence.clone(),
            refresh_skew_ms: 0,
        },
        GatewayRuntimeOptions {
            max_retry_candidates: 3,
            session_affinity_ttl: None,
            max_affinity_entries: 0,
        },
        Arc::new(move |event| captured.lock().unwrap().push(event)),
    )
    .unwrap();
    (
        spawn(gateway::router(Arc::new(runtime))).await,
        events,
        refresh,
        persistence,
    )
}

async fn spawn_upstream(replies: Vec<Reply>) -> (TestServer, UpstreamState) {
    let state = UpstreamState {
        replies: Arc::new(Mutex::new(replies.into())),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/v1/responses", post(upstream))
        .with_state(state.clone());
    (spawn(app).await, state)
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
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    state.requests.lock().unwrap().push(ObservedRequest {
        authorization: header(&headers, AUTHORIZATION.as_str()),
        chatgpt_account_id: header(&headers, "chatgpt-account-id"),
        originator: header(&headers, "originator"),
        body: serde_json::from_slice(&body).unwrap_or(Value::Null),
    });
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

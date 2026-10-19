use super::*;
use crate::providers::chatgpt::{ModelDiscoveryFailureCode, OAuthClientKind};
use crate::scheduler::rotation::SharedRequestBudget;
use axum::extract::State;
use axum::http::Uri;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use tokio::sync::{Notify, Semaphore};

mod catalog_discovery;
mod quota_rotation;
mod recovery;
mod request_timezone;
mod retry_admission;
mod routing;

struct AccessReply {
    status: StatusCode,
    body: Value,
}

struct AccessState {
    reply: Mutex<AccessReply>,
    generation_body: Mutex<Value>,
    generation_bodies: Mutex<Vec<Value>>,
    generation_replies: Mutex<std::collections::VecDeque<(StatusCode, Value)>>,
    access_requests: AtomicUsize,
    generation_requests: AtomicUsize,
    attachment_requests: AtomicUsize,
    generation_status: Mutex<StatusCode>,
    strict_request_schema: AtomicBool,
    stream_generations: AtomicBool,
    stream_gates: Mutex<std::collections::VecDeque<Arc<Semaphore>>>,
    first_started: Notify,
    first_release: Semaphore,
    hold_first: bool,
}

struct AccessServer {
    responses_url: Url,
    state: Arc<AccessState>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for AccessServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl AccessServer {
    async fn start(models: Value, hold_first: bool) -> Self {
        let state = Arc::new(AccessState {
            reply: Mutex::new(AccessReply {
                status: StatusCode::OK,
                body: json!({"allowed": true, "model_catalog": {"models": models}}),
            }),
            generation_body: Mutex::new(json!({
                "id":"synthetic-response","status":"completed","output":[]
            })),
            generation_bodies: Mutex::new(Vec::new()),
            generation_replies: Mutex::new(Default::default()),
            access_requests: AtomicUsize::new(0),
            generation_requests: AtomicUsize::new(0),
            attachment_requests: AtomicUsize::new(0),
            generation_status: Mutex::new(StatusCode::OK),
            strict_request_schema: AtomicBool::new(false),
            stream_generations: AtomicBool::new(false),
            stream_gates: Mutex::new(Default::default()),
            first_started: Notify::new(),
            first_release: Semaphore::new(0),
            hold_first,
        });
        let app = Router::new()
            .route("/responses/access", get(access))
            .route(
                "/responses",
                post(generation).get(|State(state): State<Arc<AccessState>>| async move {
                    // Count rejected upgrades so tests catch stray native WebSocket requests.
                    state.generation_requests.fetch_add(1, Ordering::SeqCst);
                    StatusCode::METHOD_NOT_ALLOWED
                }),
            )
            .route("/attachments", post(failed_attachment))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let responses_url = Url::parse(&format!(
            "http://{}/responses",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Self {
            responses_url,
            state,
            task,
        }
    }

    fn reply_with(&self, status: StatusCode, body: Value) {
        *crate::poison::mutex(&self.state.reply) = AccessReply { status, body };
    }
}

async fn failed_attachment(State(state): State<Arc<AccessState>>) -> StatusCode {
    state.attachment_requests.fetch_add(1, Ordering::SeqCst);
    StatusCode::BAD_GATEWAY
}

async fn access(State(state): State<Arc<AccessState>>, uri: Uri, headers: HeaderMap) -> Response {
    assert_eq!(uri.query(), Some("include_models=true"));
    assert!(headers.contains_key(reqwest::header::AUTHORIZATION));
    assert_eq!(
        headers["x-openai-internal-basispoints-office-host"],
        "Excel"
    );
    let first = state.access_requests.fetch_add(1, Ordering::SeqCst) == 0;
    // Capture the reply before waiting so a late old-credential response can
    // carry a different inventory from the replacement credential's response.
    let (status, body) = {
        let reply = crate::poison::mutex(&state.reply);
        (reply.status, reply.body.clone())
    };
    if first && state.hold_first {
        state.first_started.notify_one();
        state.first_release.acquire().await.unwrap().forget();
    }
    let mut response = (status, Json(body)).into_response();
    if status == StatusCode::TOO_MANY_REQUESTS {
        response
            .headers_mut()
            .insert("retry-after", HeaderValue::from_static("17"));
    }
    response
}

async fn generation(State(state): State<Arc<AccessState>>, Json(body): Json<Value>) -> Response {
    state.generation_requests.fetch_add(1, Ordering::SeqCst);
    let explicit_messages = body["input"].as_array().is_some_and(|items| {
        items.iter().all(|item| {
            item.get("role").is_none() || (item["type"] == "message" && item["content"].is_array())
        })
    });
    let transport_metadata = body["metadata"].as_object().is_some_and(|metadata| {
        metadata.len() == 3
            && metadata.iter().all(|(key, value)| {
                matches!(key.as_str(), "task_id" | "turn_id" | "agent_iteration")
                    && value.is_string()
            })
    });
    let streaming = body["stream"] == true && state.stream_generations.load(Ordering::SeqCst);
    crate::poison::mutex(&state.generation_bodies).push(body);
    if state.strict_request_schema.load(Ordering::SeqCst)
        && (!explicit_messages || !transport_metadata)
    {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"error":{"type":"server_error","message":"422: Invalid request body."}})),
        )
            .into_response();
    }
    if let Some((status, response)) = crate::poison::mutex(&state.generation_replies).pop_front() {
        return generation_reply(&state, status, response, streaming);
    }
    generation_reply(
        &state,
        *crate::poison::mutex(&state.generation_status),
        crate::poison::mutex(&state.generation_body).clone(),
        streaming,
    )
}

fn generation_reply(
    state: &AccessState,
    status: StatusCode,
    body: Value,
    streaming: bool,
) -> Response {
    if !streaming || !status.is_success() {
        return (status, Json(body)).into_response();
    }
    use futures_util::StreamExt;
    let wire =
        crate::gateway::test_support::basis_points_stream(&serde_json::to_vec(&body).unwrap());
    let mut pending = wire.clone();
    let mut split = wire.len();
    while let Some(frame) = crate::protocol::take_sse_event(&mut pending) {
        let event: Value =
            serde_json::from_slice(&crate::protocol::sse_data(&frame)).unwrap_or(Value::Null);
        if matches!(
            event["type"].as_str(),
            Some("response.output_text.delta" | "response.function_call_arguments.delta")
        ) {
            split = wire.len() - pending.len();
            break;
        }
    }
    let first = axum::body::Bytes::copy_from_slice(&wire[..split]);
    let rest = axum::body::Bytes::copy_from_slice(&wire[split..]);
    let gate = crate::poison::mutex(&state.stream_gates).pop_front();
    let stream = futures_util::stream::once(async { Ok::<_, std::convert::Infallible>(first) })
        .chain(futures_util::stream::once(async move {
            if let Some(gate) = gate {
                gate.acquire().await.unwrap().forget();
            }
            Ok(rest)
        }));
    Response::builder()
        .status(status)
        .header("content-type", "text/event-stream")
        .body(axum::body::Body::from_stream(stream))
        .unwrap()
}

struct RecoveryRefresh {
    calls: AtomicUsize,
}

impl TokenRefreshAdapter for RecoveryRefresh {
    fn refresh<'a>(
        &'a self,
        _account_id: &'a str,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> BoxFuture<'a, std::result::Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async move {
            assert_eq!(refresh_token, "synthetic-refresh");
            self.calls.fetch_add(1, Ordering::SeqCst);
            TokenRefresh::new(
                "synthetic-new-access",
                Some("synthetic-new-refresh".into()),
                None,
                Some(now_ms + 600_000),
            )
            .map_err(|_| TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "invalid"))
        })
    }
}

#[tokio::test]
async fn bps_401_retains_refresh_token_and_recovers_only_on_the_next_request() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    *crate::poison::mutex(&server.state.generation_status) = StatusCode::UNAUTHORIZED;
    let mut runtime = runtime(&server).await;
    let refresh = Arc::new(RecoveryRefresh {
        calls: AtomicUsize::new(0),
    });
    let account = runtime.chatgpt_accounts.get_mut("account-1").unwrap();
    account.refresh_adapter = refresh.clone();
    account
        .token_authority
        .register(
            "account-1",
            TokenSet::new(
                "synthetic-access",
                Some("synthetic-refresh".into()),
                None,
                Some(runtime_now_ms() + 600_000),
                runtime_now_ms(),
                1,
            )
            .unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    for status in [StatusCode::UNAUTHORIZED, StatusCode::OK] {
        *crate::poison::mutex(&server.state.generation_status) = status;
        let budget = SharedRequestBudget::for_incoming_request(3);
        let response = runtime
            .send_authorized_request(
                "account-1",
                reqwest::Client::new()
                    .post(server.responses_url.clone())
                    .json(&json!({"model":"gpt-test"})),
                AuthorizationDispatch {
                    client_version: None,
                    identity_policy: AuthorizationIdentityPolicy::PreserveUpstream,
                    turn_scope: None,
                    budget: Some(&budget),
                    lease: None,
                    usage_event: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(response.response.status(), status);
        assert_eq!(budget.dispatches(), 1);
        let tokens = runtime.chatgpt_accounts["account-1"]
            .token_authority
            .tokens("account-1")
            .await
            .unwrap();
        if status == StatusCode::UNAUTHORIZED {
            assert_eq!(refresh.calls.load(Ordering::SeqCst), 0);
            assert_eq!(tokens.refresh_token(), Some("synthetic-refresh"));
        } else {
            assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
            assert_eq!(tokens.refresh_token(), Some("synthetic-new-refresh"));
        }
    }
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
}

async fn runtime(server: &AccessServer) -> GatewayRuntime {
    let mut runtime = quota_runtime(QuotaSnapshot::default());
    let account = runtime.chatgpt_accounts.get_mut("account-1").unwrap();
    account.oauth_client_kind = OAuthClientKind::ExcelBps;
    account.basis_points_url = server.responses_url.clone();
    account
        .token_authority
        .register(
            "account-1",
            TokenSet::access_only("synthetic-access", None, 1).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    runtime
}

fn expire_access(runtime: &GatewayRuntime) {
    crate::poison::write(&runtime.chatgpt_accounts["account-1"].basis_points_access)
        .as_mut()
        .unwrap()
        .expire();
}

#[tokio::test]
async fn free_bps_rejects_discovery_and_all_dispatches_without_provider_requests() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let runtime = runtime(&server).await;
    assert!(runtime.update_account_subscription_plan("account-1", Some("chatgpt_free_tier")));
    let result = runtime
        .prepare_basis_points_authorization("account-1")
        .await;
    assert!(
        matches!(result, Err(AuthorizedRequestError::ModelAccess(failure))
        if failure.code == ModelDiscoveryFailureCode::BasisPointsFreeAccountUnsupported)
    );
    let key = runtime.authenticate_secret("local-secret").unwrap();
    runtime.refresh_basis_points_access(&key).await;
    let budget = SharedRequestBudget::for_incoming_request(3);
    for request_budget in [Some(&budget), None] {
        let result = runtime
            .send_authorized_request(
                "account-1",
                reqwest::Client::new()
                    .post(server.responses_url.clone())
                    .json(&json!({"model":"gpt-test"})),
                AuthorizationDispatch {
                    client_version: None,
                    identity_policy: AuthorizationIdentityPolicy::PreserveUpstream,
                    turn_scope: None,
                    budget: request_budget,
                    lease: None,
                    usage_event: None,
                },
            )
            .await;
        assert!(
            matches!(result, Err(AuthorizedRequestError::ModelAccess(failure))
            if failure.code == ModelDiscoveryFailureCode::BasisPointsFreeAccountUnsupported)
        );
    }
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|budget| budget.wire_attempts()), 0);
    assert_eq!(server.state.access_requests.load(Ordering::SeqCst), 0);
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 0);
}

struct FreePlanRefresh {
    calls: AtomicUsize,
}

impl TokenRefreshAdapter for FreePlanRefresh {
    fn refresh<'a>(
        &'a self,
        _account_id: &'a str,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> BoxFuture<'a, std::result::Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async move {
            use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
            assert_eq!(refresh_token, "synthetic-refresh");
            self.calls.fetch_add(1, Ordering::SeqCst);
            let id_token = format!(
                "e30.{}.synthetic",
                URL_SAFE_NO_PAD
                    .encode(br#"{"https://api.openai.com/auth":{"chatgpt_plan_type":"free"}}"#)
            );
            Ok(TokenRefresh::new(
                "synthetic-refreshed-access",
                None,
                Some(id_token),
                Some(now_ms + 120_000),
            )
            .unwrap())
        })
    }
}

#[tokio::test]
async fn refreshed_free_id_token_blocks_bps_despite_paid_metadata_and_opaque_access() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let mut runtime = runtime(&server).await;
    runtime.update_account_subscription_plan("account-1", Some("plus"));
    let refresh = Arc::new(FreePlanRefresh {
        calls: AtomicUsize::new(0),
    });
    let account = runtime.chatgpt_accounts.get_mut("account-1").unwrap();
    account.refresh_adapter = refresh.clone();
    account
        .token_authority
        .register(
            "account-1",
            TokenSet::new(
                "synthetic-expired-access",
                Some("synthetic-refresh".into()),
                None,
                Some(1),
                0,
                1,
            )
            .unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let result = runtime
        .prepare_basis_points_authorization("account-1")
        .await;
    assert!(
        matches!(result, Err(AuthorizedRequestError::ModelAccess(failure))
        if failure.code == ModelDiscoveryFailureCode::BasisPointsFreeAccountUnsupported)
    );
    let prepared = runtime
        .prepare_authorization("account-1", crate::unix_time_ms())
        .await
        .unwrap();
    assert!(prepared.dispatch_guard(&runtime, "account-1").is_none());
    let budget = SharedRequestBudget::for_incoming_request(3);
    for request_budget in [Some(&budget), None] {
        let result = runtime
            .send_authorized_request(
                "account-1",
                reqwest::Client::new()
                    .post(server.responses_url.clone())
                    .json(&json!({"model":"gpt-test"})),
                AuthorizationDispatch {
                    client_version: None,
                    identity_policy: AuthorizationIdentityPolicy::PreserveUpstream,
                    turn_scope: None,
                    budget: request_budget,
                    lease: None,
                    usage_event: None,
                },
            )
            .await;
        assert!(
            matches!(result, Err(AuthorizedRequestError::ModelAccess(failure))
            if failure.code == ModelDiscoveryFailureCode::BasisPointsFreeAccountUnsupported)
        );
    }
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|budget| budget.wire_attempts()), 0);
    assert_eq!(server.state.access_requests.load(Ordering::SeqCst), 0);
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn bps_plan_downgrade_revokes_cached_access_and_pending_dispatch() {
    use crate::scheduler::rotation::DispatchStartError;
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let runtime = runtime(&server).await;
    runtime.update_account_subscription_plan("account-1", Some("plus"));
    let prepared = runtime
        .prepare_basis_points_authorization("account-1")
        .await
        .unwrap();
    let key = runtime.authenticate_secret("local-secret").unwrap();
    let budget = SharedRequestBudget::for_incoming_request(3);
    let lease = runtime
        .select_and_reserve_with_budget(
            &key,
            "gpt-test",
            &[WireApi::Responses],
            &HashSet::new(),
            (None, None),
            crate::unix_time_ms(),
            &budget,
        )
        .await
        .unwrap()
        .1;
    assert!(runtime.update_account_subscription_plan("account-1", Some("free")));
    assert!(runtime.fresh_basis_points_access("account-1").is_none());
    assert_eq!(
        lease.begin_rotation_http_dispatch_for(&prepared, &runtime),
        Err(DispatchStartError::CandidateChanged)
    );
    assert!(runtime.update_account_policy(
        "account-1",
        RuntimeCandidatePolicy {
            enabled: true,
            draining: false,
            priority: 0,
            weight: 1,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
        }
    ));
    assert!(runtime.sync_account_availability_with_quota(
        "account-1",
        true,
        CandidateHealth::Healthy,
        &QuotaSnapshot::default(),
        crate::unix_time_ms(),
    ));
    assert!(
        !runtime
            .lock_scheduler()
            .candidate("account-1")
            .unwrap()
            .enabled
    );
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|budget| budget.wire_attempts()), 0);
    drop(lease);
    runtime.update_account_subscription_plan("account-1", Some("plus"));
    runtime.sync_account_availability_with_quota(
        "account-1",
        true,
        CandidateHealth::Healthy,
        &QuotaSnapshot::default(),
        crate::unix_time_ms(),
    );
    assert!(
        runtime
            .lock_scheduler()
            .candidate("account-1")
            .unwrap()
            .enabled
    );
    runtime
        .prepare_basis_points_authorization("account-1")
        .await
        .unwrap();
    assert_eq!(server.state.access_requests.load(Ordering::SeqCst), 2);
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn delayed_access_cannot_publish_after_a_free_plan_downgrade() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), true).await;
    let runtime = Arc::new(runtime(&server).await);
    let reading = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .prepare_basis_points_authorization("account-1")
                .await
        })
    };
    tokio::time::timeout(
        Duration::from_secs(2),
        server.state.first_started.notified(),
    )
    .await
    .unwrap();
    runtime.update_account_subscription_plan("account-1", Some("free"));
    server.state.first_release.add_permits(1);
    assert!(reading.await.unwrap().is_err());
    assert!(runtime.fresh_basis_points_access("account-1").is_none());
    assert!(
        !runtime
            .lock_scheduler()
            .candidate("account-1")
            .unwrap()
            .enabled
    );
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn native_free_account_keeps_its_route_and_never_uses_bps_access() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let mut runtime = runtime(&server).await;
    runtime
        .chatgpt_accounts
        .get_mut("account-1")
        .unwrap()
        .oauth_client_kind = OAuthClientKind::Codex;
    runtime.update_account_subscription_plan("account-1", Some("free"));
    assert!(
        runtime
            .lock_scheduler()
            .candidate("account-1")
            .unwrap()
            .enabled
    );
    let budget = SharedRequestBudget::for_incoming_request(1);
    let response = runtime
        .send_authorized_request(
            "account-1",
            reqwest::Client::new()
                .post(server.responses_url.clone())
                .json(&json!({"model":"gpt-test"})),
            AuthorizationDispatch {
                client_version: None,
                identity_policy: AuthorizationIdentityPolicy::PreserveUpstream,
                turn_scope: None,
                budget: Some(&budget),
                lease: None,
                usage_event: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(response.response.status(), StatusCode::OK);
    assert_eq!(server.state.access_requests.load(Ordering::SeqCst), 0);
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn concurrent_access_reads_share_one_discovery_and_publish_inventory() {
    let server = AccessServer::start(
        json!([{"id":"gpt-test"}, {"id":"gpt-new", "efforts":[{"value":"xhigh"}]}]),
        false,
    )
    .await;
    let runtime = runtime(&server).await;
    let key = runtime.authenticate_secret("local-secret").unwrap();
    assert!(runtime.visible_account_models(&key).is_empty());
    let reads = futures_util::future::join_all(
        (0..12).map(|_| runtime.prepare_basis_points_authorization("account-1")),
    )
    .await;
    assert!(reads.iter().all(|result| result.is_ok()));
    assert_eq!(server.state.access_requests.load(Ordering::SeqCst), 1);
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 0);
    assert_eq!(
        runtime.visible_account_models(&key),
        ["gpt-new", "gpt-test"]
    );
    assert_eq!(
        runtime
            .visible_models(&key, &[WireApi::Responses], crate::unix_time_ms())
            .len(),
        2
    );
    assert!(runtime.basis_points_reasoning_available("account-1", "gpt-new", "max"));
    assert!(!runtime.basis_points_reasoning_available("account-1", "gpt-new", "medium"));
    assert!(runtime.basis_points_reasoning_available("unrelated-source", "gpt-new", "medium"));
}

#[tokio::test]
async fn token_replacement_invalidates_access_even_with_identical_visible_fields() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let runtime = runtime(&server).await;
    runtime
        .prepare_basis_points_authorization("account-1")
        .await
        .unwrap();
    let key = runtime.authenticate_secret("local-secret").unwrap();
    let authority = &runtime.chatgpt_accounts["account-1"].token_authority;
    let original = authority.tokens("account-1").await.unwrap();
    authority
        .register("account-1", original, AccountAuthState::Active)
        .await
        .unwrap();
    assert!(runtime.visible_account_models(&key).is_empty());
    assert!(runtime.fresh_basis_points_access("account-1").is_none());
    server.reply_with(
        StatusCode::OK,
        json!({"allowed":true,"model_catalog":{"models":[{"id":"gpt-replacement"}]}}),
    );
    runtime
        .prepare_basis_points_authorization("account-1")
        .await
        .unwrap();
    assert_eq!(server.state.access_requests.load(Ordering::SeqCst), 2);
    assert_eq!(runtime.visible_account_models(&key), ["gpt-replacement"]);
}

#[tokio::test]
async fn delayed_old_credential_response_cannot_publish_access_or_inventory() {
    let server = AccessServer::start(json!([{"id":"gpt-stale"}]), true).await;
    let runtime = Arc::new(runtime(&server).await);
    let reading = {
        let runtime = runtime.clone();
        tokio::spawn(async move {
            runtime
                .prepare_basis_points_authorization("account-1")
                .await
        })
    };
    tokio::time::timeout(
        Duration::from_secs(2),
        server.state.first_started.notified(),
    )
    .await
    .unwrap();
    runtime.chatgpt_accounts["account-1"]
        .token_authority
        .register(
            "account-1",
            TokenSet::access_only("synthetic-replacement", None, 2).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    server.state.first_release.add_permits(1);
    assert!(matches!(
        reading.await.unwrap(),
        Err(AuthorizedRequestError::ModelAccess(failure))
            if failure.code == ModelDiscoveryFailureCode::Transport
    ));
    assert!(runtime.fresh_basis_points_access("account-1").is_none());
    assert_eq!(
        runtime
            .lock_scheduler()
            .candidate("account-1")
            .unwrap()
            .models,
        BTreeSet::from(["gpt-test".to_string()])
    );
}

#[tokio::test]
async fn failed_refresh_hides_stale_access_preserves_inventory_and_respects_retry_after() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let runtime = runtime(&server).await;
    runtime
        .prepare_basis_points_authorization("account-1")
        .await
        .unwrap();
    expire_access(&runtime);
    let key = runtime.authenticate_secret("local-secret").unwrap();
    assert!(runtime.visible_account_models(&key).is_empty());
    server.reply_with(StatusCode::TOO_MANY_REQUESTS, json!({"error":"synthetic"}));
    let reads = futures_util::future::join_all(
        (0..8).map(|_| runtime.prepare_basis_points_authorization("account-1")),
    )
    .await;
    assert!(reads.into_iter().all(|result| matches!(
        result,
        Err(AuthorizedRequestError::ModelAccess(failure))
            if failure.code == ModelDiscoveryFailureCode::RateLimited
                && failure.retry_after_ms.is_some_and(|delay| (1..=17_000).contains(&delay))
    )));
    assert_eq!(server.state.access_requests.load(Ordering::SeqCst), 2);
    assert!(runtime.visible_account_models(&key).is_empty());
    assert_eq!(
        runtime
            .lock_scheduler()
            .candidate("account-1")
            .unwrap()
            .models,
        BTreeSet::from(["gpt-test".to_string()])
    );
}

#[tokio::test]
async fn unavailable_model_or_reasoning_fails_before_spending_a_generation() {
    let server = AccessServer::start(
        json!([{"id":"gpt-test","efforts":[{"value":"xhigh"}]}]),
        false,
    )
    .await;
    let runtime = runtime(&server).await;
    let client = reqwest::Client::new();
    for (body, denied_model) in [
        (json!({"model":"gpt-denied"}), true),
        (
            json!({"model":"gpt-test", "reasoning_effort":"medium"}),
            false,
        ),
    ] {
        let budget = SharedRequestBudget::for_incoming_request(3);
        let result = runtime
            .send_authorized_request(
                "account-1",
                client.post(server.responses_url.clone()).json(&body),
                AuthorizationDispatch {
                    client_version: None,
                    identity_policy: AuthorizationIdentityPolicy::PreserveUpstream,
                    turn_scope: None,
                    budget: Some(&budget),
                    lease: None,
                    usage_event: None,
                },
            )
            .await;
        assert!(
            matches!(&result, Err(AuthorizedRequestError::ModelUnavailable)) && denied_model
                || matches!(&result, Err(AuthorizedRequestError::ReasoningUnavailable))
                    && !denied_model
        );
        assert_eq!(budget.dispatches(), 0);
        assert_eq!(budget.with_budget(|budget| budget.wire_attempts()), 0);
    }
    assert_eq!(server.state.access_requests.load(Ordering::SeqCst), 1);
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn discovery_and_alias_validation_spend_only_the_actual_generation_budget() {
    let server = AccessServer::start(
        json!([{"id":"gpt-test","efforts":[{"value":"max"}]}]),
        false,
    )
    .await;
    let runtime = runtime(&server).await;
    let budget = SharedRequestBudget::for_incoming_request(1);
    let response = runtime
        .send_authorized_request(
            "account-1",
            reqwest::Client::new()
                .post(server.responses_url.clone())
                .json(&json!({"model":"gpt-test","reasoning_effort":"xhigh"})),
            AuthorizationDispatch {
                client_version: None,
                identity_policy: AuthorizationIdentityPolicy::PreserveUpstream,
                turn_scope: None,
                budget: Some(&budget),
                lease: None,
                usage_event: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(response.response.status(), StatusCode::OK);
    assert_eq!(server.state.access_requests.load(Ordering::SeqCst), 1);
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
    assert_eq!(budget.dispatches(), 1);
    assert_eq!(budget.with_budget(|budget| budget.wire_attempts()), 1);
}

#[tokio::test]
async fn completed_tool_validation_failure_does_not_repeat_pool_or_account_execution() {
    for account_only in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        *crate::poison::mutex(&server.state.generation_body) = json!({
            "id":"synthetic-response","status":"completed","model":"gpt-test",
            "output":[{
                "type":"function_call","name":"run_officejs","call_id":"synthetic-call",
                "arguments":json!({"references":[],"code":"{}"}).to_string()
            }],
            "usage":{"input_tokens":2,"output_tokens":1,"total_tokens":3}
        });
        let runtime = Arc::new(runtime(&server).await);
        let body = if account_only {
            let response = crate::gateway::execute_account_wake(
                runtime,
                crate::gateway::AccountWakeRequest {
                    local_key_id: "key-1".to_string(),
                    account_id: "account-1".to_string(),
                    model_id: "gpt-test".to_string(),
                    output_token_cap: 8,
                },
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
            axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap()
        } else {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let gateway = tokio::spawn(async move {
                axum::serve(listener, crate::gateway::router(runtime))
                    .await
                    .unwrap()
            });
            let response = reqwest::Client::new()
                .post(format!("http://{address}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&json!({"model":"gpt-test","input":"synthetic request"}))
                .send()
                .await
                .unwrap();
            let status = response.status();
            let body = response.bytes().await.unwrap();
            gateway.abort();
            assert_eq!(status, StatusCode::BAD_GATEWAY);
            body
        };
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["error"]["code"],
            crate::error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID
        );
        assert_eq!(server.state.access_requests.load(Ordering::SeqCst), 1);
        assert_eq!(
            server.state.generation_requests.load(Ordering::SeqCst),
            1,
            "completed execution must be terminal, account_only={account_only}"
        );
    }
}

use super::*;

pub(super) fn bounded_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap()
}

pub(super) async fn pool_runtime(
    server: &AccessServer,
    bps_count: usize,
    native: bool,
) -> GatewayRuntime {
    let authority = Arc::new(TokenAuthority::new(bps_count + usize::from(native)).unwrap());
    let mut accounts = Vec::new();
    for index in 0..bps_count + usize::from(native) {
        let mut account = quota_account(QuotaSnapshot::default());
        account.id = format!("synthetic-account-{index}");
        account.chatgpt_account_id = format!("synthetic-provider-{index}");
        account.responses_url = server.responses_url.to_string();
        if index < bps_count {
            account.oauth_client_kind = OAuthClientKind::ExcelBps;
            account.priority = 100;
        }
        authority
            .register(
                &account.id,
                TokenSet::access_only("synthetic-access", None, 1).unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        accounts.push(account);
    }
    let mut runtime = GatewayRuntime::from_mixed_pool(
        Vec::new(),
        accounts,
        vec![RuntimeMixedLocalKey {
            key: key("key-1", "local-secret"),
            enabled: true,
            source_ids: None,
            account_ids: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
            wire_apis: None,
        }],
        RuntimeChatGptAuth {
            token_authority: authority,
            refresh_adapter: Arc::new(NeverRefresh),
            persistence_adapter: Arc::new(NoopPersistence),
            refresh_skew_ms: 60_000,
            agent_identities: HashMap::new(),
        },
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    for account in runtime.chatgpt_accounts.values_mut() {
        account.basis_points_url = server.responses_url.clone();
    }
    runtime
}

/// Retire only the synthetic runtime if the old loop starts spinning. A timer
/// alone cannot reliably interrupt a loop whose awaits are all immediately ready.
fn observe_reservations(runtime: &Arc<GatewayRuntime>) -> Arc<AtomicUsize> {
    let events = Arc::new(AtomicUsize::new(0));
    let captured = events.clone();
    let weak = Arc::downgrade(runtime);
    runtime.set_activity_callback(move |_| {
        if captured.fetch_add(1, Ordering::SeqCst) >= 16
            && let Some(runtime) = weak.upgrade()
        {
            runtime.retire_for_replacement();
        }
    });
    events
}

#[tokio::test]
async fn incompatible_bps_preparation_does_not_restart_recovery_or_access_reads() {
    for persistent in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        let usage = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = usage.clone();
        let mut runtime = runtime(&server).await;
        runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        let runtime = Arc::new(runtime);
        runtime.set_route_recovery_enabled(persistent);
        let events = observe_reservations(&runtime);
        let (base, task) = super::recovery::gateway(runtime.clone()).await;
        let response = bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({
                "model":"gpt-test", "input":"private-preparation-sentinel",
                "text":{"format":{"type":"json_object"}}
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], "adapter_parameter_unsupported");
        assert!(
            events.load(Ordering::SeqCst) <= 2,
            "local incompatibility recycled its reservation"
        );
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 0);
        assert_eq!(server.state.access_requests.load(Ordering::SeqCst), 1);
        assert!(!runtime.lock_scheduler().is_retired());
        let usage = crate::poison::mutex(&usage);
        assert_eq!(usage.len(), 1);
        let event = &usage[0];
        assert_eq!(event.attempt, 0);
        assert_eq!(event.source_id, "relay");
        assert!(event.candidate_id.is_none());
        assert!(event.account_id.is_none());
        assert!(event.total_tokens.is_none());
        assert!(event.cooldown_scope.is_none());
        let details = event.upstream_error.as_ref().unwrap();
        assert_eq!(details.http_status, Some(400));
        assert_eq!(
            details.code.as_deref(),
            Some("adapter_parameter_unsupported")
        );
        assert_eq!(details.error_type.as_deref(), Some("relay_adapter"));
        assert_eq!(
            details.parameter.as_deref(),
            body["error"]["param"].as_str()
        );
        assert!(
            details
                .message
                .as_ref()
                .unwrap()
                .contains(details.parameter.as_ref().unwrap())
        );
        assert!(
            !serde_json::to_string(event)
                .unwrap()
                .contains("private-preparation-sentinel")
        );
        task.abort();
    }
}

#[tokio::test]
async fn incompatible_bps_compact_endpoint_does_not_restart_account_recovery() {
    for persistent in [false, true] {
        for count in [1, 5] {
            let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
            let runtime = Arc::new(pool_runtime(&server, count, false).await);
            runtime.set_route_recovery_enabled(persistent);
            let events = observe_reservations(&runtime);
            let (base, task) = super::recovery::gateway(runtime.clone()).await;
            let response = bounded_client()
                .post(format!("{base}/v1/responses/compact"))
                .bearer_auth("local-secret")
                .json(&json!({"model":"gpt-test", "input":"Synthetic input"}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            assert_eq!(events.load(Ordering::SeqCst), count * 2);
            assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 0);
            assert!(!runtime.lock_scheduler().is_retired());
            task.abort();
        }
    }
}

#[tokio::test]
async fn incompatible_bps_pool_does_not_block_a_later_compatible_request() {
    for persistent in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        let runtime = Arc::new(pool_runtime(&server, 5, false).await);
        runtime.set_route_recovery_enabled(persistent);
        let events = observe_reservations(&runtime);
        let (base, task) = super::recovery::gateway(runtime.clone()).await;
        let client = bounded_client();
        let response = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({
                "model":"gpt-test", "input":"Synthetic input",
                "text":{"format":{"type":"json_object"}}
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(events.load(Ordering::SeqCst), 0);
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 0);
        assert_eq!(server.state.access_requests.load(Ordering::SeqCst), 5);
        {
            let scheduler = runtime.lock_scheduler();
            for index in 0..5 {
                let account = scheduler
                    .candidate(&format!("synthetic-account-{index}"))
                    .unwrap();
                assert!(account.enabled);
                assert!(account.cooldowns.is_empty());
            }
        }
        let response = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({"model":"gpt-test", "input":"Synthetic input"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
        assert_eq!(server.state.access_requests.load(Ordering::SeqCst), 5);
        assert!(!runtime.lock_scheduler().is_retired());
        task.abort();
    }
}

#[tokio::test]
async fn incompatible_bps_route_can_fall_back_to_a_compatible_native_account() {
    for bps_count in [1, 16] {
        for client in [WireApi::Responses, WireApi::ChatCompletions] {
            let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
            *crate::poison::mutex(&server.state.generation_body) = json!({
                "id":"synthetic-review","model":"gpt-test","status":"completed",
                "output":[{"type":"message","role":"assistant","content":[{
                    "type":"output_text","text":"{\"approved\":false}"
                }]}]
            });
            let runtime = Arc::new(pool_runtime(&server, bps_count, true).await);
            runtime.set_route_recovery_enabled(true);
            let events = observe_reservations(&runtime);
            let (base, task) = super::recovery::gateway(runtime.clone()).await;
            let schema = json!({
                "name":"command_review","strict":true,"schema":{
                    "type":"object","properties":{"approved":{"type":"boolean"}},
                    "required":["approved"],"additionalProperties":false
                }
            });
            let mut format = schema.clone();
            format["type"] = json!("json_schema");
            let (path, request) = match client {
                WireApi::Responses => (
                    "/v1/responses",
                    json!({
                        "model":"gpt-test","input":"Review a synthetic command",
                        "text":{"format":format}
                    }),
                ),
                _ => (
                    "/v1/chat/completions",
                    json!({
                        "model":"gpt-test","messages":[{"role":"user","content":"Review a synthetic command"}],
                        "response_format":{"type":"json_schema","json_schema":schema}
                    }),
                ),
            };
            let response = bounded_client()
                .post(format!("{base}{path}"))
                .bearer_auth("local-secret")
                .json(&request)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let body: Value = response.json().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{body}");
            let answer = match client {
                WireApi::Responses => &body["output"][0]["content"][0]["text"],
                _ => &body["choices"][0]["message"]["content"],
            };
            assert_eq!(answer, "{\"approved\":false}");
            {
                let bodies = crate::poison::mutex(&server.state.generation_bodies);
                assert_eq!(bodies.len(), 1);
                assert_eq!(bodies[0]["text"]["format"], format);
            }
            assert_eq!(events.load(Ordering::SeqCst), 2);
            // Catalog discovery is independent of execution admission.
            assert_eq!(
                server.state.access_requests.load(Ordering::SeqCst),
                bps_count
            );
            assert!(!runtime.lock_scheduler().is_retired());
            task.abort();
        }
    }
}

#[tokio::test]
async fn failed_bps_upload_does_not_restart_pre_dispatch_recovery() {
    for persistent in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        let runtime = Arc::new(pool_runtime(&server, 5, false).await);
        runtime.set_route_recovery_enabled(persistent);
        let events = observe_reservations(&runtime);
        let (base, task) = super::recovery::gateway(runtime.clone()).await;
        let response = bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({
                "model":"gpt-test", "input":[{
                    "role":"user", "content":[{
                        "type":"input_image",
                        "image_url":"data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
                    }]
                }]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(server.state.attachment_requests.load(Ordering::SeqCst), 5);
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 0);
        assert_eq!(server.state.access_requests.load(Ordering::SeqCst), 5);
        assert_eq!(events.load(Ordering::SeqCst), 10);
        assert!(!runtime.lock_scheduler().is_retired());
        task.abort();
    }
}

use super::*;

pub(super) async fn gateway(runtime: Arc<GatewayRuntime>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, crate::gateway::router(runtime))
            .await
            .unwrap();
    });
    (base, task)
}

async fn request(base: &str, model: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&json!({"model":model,"input":"synthetic input"}))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn invalid_bps_body_422_is_returned_once_without_cooling_the_account() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    *crate::poison::mutex(&server.state.generation_status) = StatusCode::UNPROCESSABLE_ENTITY;
    *crate::poison::mutex(&server.state.generation_body) =
        json!({"error":{"type":"server_error","message":"422: Invalid request body."}});
    let runtime = Arc::new(runtime(&server).await);
    let (base, task) = gateway(runtime.clone()).await;
    let response = request(&base, "gpt-test").await;
    assert!(response.status().is_client_error());
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
    let scheduler = runtime.lock_scheduler();
    let account = scheduler.candidate("account-1").unwrap();
    assert!(account.cooldowns.is_empty());
    assert!(account.enabled);
    drop(scheduler);
    task.abort();
}

#[tokio::test]
async fn claude_messages_satisfy_strict_bps_schema_through_json_and_streaming_tool_cycles() {
    for stream in [false, true] {
        claude_tool_cycle(stream).await;
    }
}

#[tokio::test]
async fn claude_desktop_effort_is_separate_from_model_and_obeys_pool_policy() {
    use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};

    let server = AccessServer::start(
        json!([{"id":"gpt-test","efforts":[
            {"value":"low"},{"value":"medium"},{"value":"high"},
            {"value":"xhigh"},{"value":"max"}
        ]}]),
        false,
    )
    .await;
    *crate::poison::mutex(&server.state.generation_body) = json!({
        "id":"resp_synthetic_effort","object":"response","status":"completed",
        "model":"gpt-test","output":[{
            "type":"message","id":"msg_synthetic","role":"assistant","status":"completed",
            "content":[{"type":"output_text","text":"Ready","annotations":[]}]
        }],"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}
    });
    let mut runtime = runtime(&server).await;
    runtime.model_metadata_catalog = Some(ModelMetadataCatalogHandle::new(
        ModelMetadataCatalog::from_models_dev_json(
            r#"{"test/gpt-test":{"reasoning":true,"reasoning_effort_levels":["low","medium","high","xhigh","max"]}}"#,
        )
        .unwrap(),
    ));
    let runtime = Arc::new(runtime);
    let alias = crate::claude_desktop_reasoning_model_alias("gpt-test");
    let (base, task) = gateway(runtime.clone()).await;
    let client = reqwest::Client::new();
    for stream in [false, true] {
        for effort in [
            None,
            Some("low"),
            Some("medium"),
            Some("high"),
            Some("xhigh"),
            Some("max"),
        ] {
            let mut request = json!({
                "model":alias,"max_tokens":128,"stream":stream,
                "messages":[{"role":"user","content":"Synthetic request"}]
            });
            if let Some(effort) = effort {
                request["output_config"] = json!({"effort":effort});
                request["thinking"] = json!({"type":"adaptive"});
            }
            let response = client
                .post(format!("{base}/v1/messages"))
                .bearer_auth("local-secret")
                .json(&request)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{stream} {effort:?}");
            claude_response_body(response, stream).await;
            let bodies = crate::poison::mutex(&server.state.generation_bodies);
            let upstream = bodies.last().unwrap();
            assert_eq!(upstream["model"], "gpt-test");
            // BPS's existing wire codec spells its advertised `max` as
            // `xhigh`. This transport alias is independent of Desktop's UI ID.
            let wire_effort = if effort == Some("max") {
                Some("xhigh")
            } else {
                effort
            };
            assert_eq!(upstream["reasoning_effort"].as_str(), wire_effort);
            assert!(upstream.get("output_config").is_none());
        }
    }
    runtime
        .set_model_reasoning_allowed_levels(BTreeMap::from([(
            "gpt-test".into(),
            vec!["low".into()],
        )]))
        .unwrap();
    for (model, expected) in [
        (alias.clone(), StatusCode::OK),
        ("gpt-test".to_string(), StatusCode::BAD_REQUEST),
        (
            crate::claude_desktop_model_alias("gpt-test"),
            StatusCode::BAD_REQUEST,
        ),
        (format!("{alias}-effort-high"), StatusCode::NOT_FOUND),
    ] {
        let response = client
            .post(format!("{base}/v1/messages"))
            .bearer_auth("local-secret")
            .json(&json!({
                "model":model,"max_tokens":128,"output_config":{"effort":"high"},
                "messages":[{"role":"user","content":"Synthetic request"}]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        response.bytes().await.unwrap();
    }
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 13);
    let bodies = crate::poison::mutex(&server.state.generation_bodies);
    // The fixed picker stays within pool policy; ordinary API model IDs remain strict.
    assert_eq!(bodies.last().unwrap()["model"], "gpt-test");
    assert_eq!(bodies.last().unwrap()["reasoning_effort"], "low");
    drop(bodies);
    task.abort();
}

async fn claude_tool_cycle(stream: bool) {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    server
        .state
        .stream_generations
        .store(stream, Ordering::SeqCst);
    server
        .state
        .strict_request_schema
        .store(true, Ordering::SeqCst);
    let runtime = Arc::new(runtime(&server).await);
    let (base, task) = gateway(runtime).await;
    let mut history = vec![json!({"role":"user","content":"Hello"})];
    for turn in 0..3 {
        let output = if turn == 1 {
            json!([
                {"type":"web_search_call","id":"ws_synthetic","status":"completed"},
                {"type":"function_call","id":"fc_lookup","call_id":"call_lookup","name":"run_officejs","arguments":json!({"references":["lookup"],"code":"{\"path\":\"synthetic\"}"}).to_string(),"status":"completed"}
            ])
        } else {
            json!([{"type":"message","id":"msg_synthetic","role":"assistant","status":"completed","content":[{"type":"output_text","text":"Ready","annotations":[]}]}])
        };
        *crate::poison::mutex(&server.state.generation_body) = json!({
            "id":format!("resp_synthetic_{turn}"),"object":"response","status":"completed","model":"gpt-test","output":output,
            "usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}
        });
        if stream && turn == 1 {
            crate::poison::mutex(&server.state.stream_prefixes).push_back(
                b"data: {\"type\":\"response.web_search_call.searching\",\"output_index\":0,\"item_id\":\"ws_synthetic\"}\n\n".to_vec(),
            );
        }
        let response = reqwest::Client::new()
            .post(format!("{base}/v1/messages"))
            .bearer_auth("local-secret")
            .json(&json!({
                "model":crate::claude_desktop_model_alias("gpt-test"),"max_tokens":128,"stream":stream,
                "metadata":{"user_id":"synthetic-user"},
                "system":[{"type":"text","text":"Use the supplied tool","cache_control":{"type":"ephemeral"}}],
                "messages":history,
                "tools":[{"name":"lookup","input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}]
            }))
            .send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "turn {turn}");
        let body = claude_response_body(response, stream).await;
        assert_eq!(
            body["content"][0]["type"],
            if turn == 1 { "tool_use" } else { "text" }
        );
        if turn == 1 {
            assert_eq!(body["content"][0]["name"], "lookup");
            assert_eq!(body["content"][0]["input"], json!({"path":"synthetic"}));
            assert_eq!(body["stop_reason"], "tool_use");
        } else {
            assert_eq!(body["content"][0]["text"], "Ready");
            assert_eq!(body["stop_reason"], "end_turn");
        }
        history.push(json!({"role":"assistant","content":body["content"]}));
        if turn == 0 {
            history.push(json!({"role":"user","content":"Look up the path"}));
        } else if turn == 1 {
            history.push(json!({"role":"user","content":[{"type":"tool_result","tool_use_id":body["content"][0]["id"],"content":"Synthetic result"}]}));
        }
    }
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 3);
    let bodies = crate::poison::mutex(&server.state.generation_bodies);
    assert!(bodies
        .iter()
        .all(|body| body["metadata"].get("user_id").is_none()
            && body["metadata"].as_object().unwrap().len() == 3));
    let final_input = bodies[2]["input"].as_array().unwrap();
    assert!(
        final_input
            .iter()
            .any(|item| item["type"] == "function_call_output"
                && item["output"] == "Synthetic result")
    );
    drop(bodies);
    task.abort();
}

async fn claude_response_body(response: reqwest::Response, stream: bool) -> Value {
    if !stream {
        return response.json().await.unwrap();
    }
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let body = response.text().await.unwrap();
    let events: Vec<Value> = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "message_stop")
            .count(),
        1
    );
    assert!(events.iter().all(|event| event["type"] != "error"));
    let mut content: Vec<Value> = Vec::new();
    let mut arguments = String::new();
    let mut stop_reason = Value::Null;
    for event in &events {
        match event["type"].as_str().unwrap() {
            "content_block_start" => content.push(event["content_block"].clone()),
            "content_block_delta" => match event["delta"]["type"].as_str().unwrap() {
                "text_delta" => {
                    let text = content[0]["text"].as_str().unwrap().to_string();
                    content[0]["text"] = json!(text + event["delta"]["text"].as_str().unwrap());
                }
                "input_json_delta" => {
                    arguments.push_str(event["delta"]["partial_json"].as_str().unwrap());
                }
                kind => panic!("unexpected synthetic delta: {kind}"),
            },
            "message_delta" => stop_reason = event["delta"]["stop_reason"].clone(),
            _ => {}
        }
    }
    assert_eq!(content.len(), 1);
    if !arguments.is_empty() {
        content[0]["input"] = serde_json::from_str(&arguments).unwrap();
    }
    json!({"content":content,"stop_reason":stop_reason})
}

#[tokio::test]
async fn claude_streaming_uses_explicit_bps_messages_and_completes() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    server
        .state
        .strict_request_schema
        .store(true, Ordering::SeqCst);
    *crate::poison::mutex(&server.state.generation_body) = json!({
        "id":"resp_stream","object":"response","status":"completed","model":"gpt-test",
        "output":[{"type":"message","id":"msg_stream","role":"assistant","status":"completed","content":[{"type":"output_text","text":"Synthetic reply","annotations":[]}]}],
        "usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}
    });
    let (base, task) = gateway(Arc::new(runtime(&server).await)).await;
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/messages"))
        .bearer_auth("local-secret")
        .json(&json!({"model":crate::claude_desktop_model_alias("gpt-test"),"max_tokens":128,"stream":true,"metadata":{"user_id":"synthetic-user"},"messages":[{"role":"user","content":"Hello"}]}))
        .send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let body = response.text().await.unwrap();
    assert!(body.contains("Synthetic reply"));
    assert!(body.contains("event: message_stop"));
    assert!(!body.contains("event: error"));
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn permission_refusal_pauses_only_the_canonical_bps_model_for_thirty_minutes() {
    for code in ["basispoints_model_access_changed", "model_not_allowed"] {
        let server =
            AccessServer::start(json!([{"id":"gpt-test"},{"id":"gpt-other"}]), false).await;
        let runtime = Arc::new(runtime(&server).await);
        runtime
            .prepare_basis_points_authorization("account-1")
            .await
            .unwrap();
        let (base, task) = gateway(runtime.clone()).await;
        *crate::poison::mutex(&server.state.generation_status) = StatusCode::FORBIDDEN;
        *crate::poison::mutex(&server.state.generation_body) = json!({"error":{"code":code}});
        let before = runtime_now_ms();
        let response = request(&base, &crate::codex_model_alias("gpt-test")).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let candidate = runtime
            .lock_scheduler()
            .candidate("account-1")
            .unwrap()
            .clone();
        let retry = candidate.cooldowns["gpt-test"];
        assert!(retry >= before + 30 * 60_000);
        assert!(retry <= runtime_now_ms() + 30 * 60_000);
        assert!(!candidate.cooldowns.contains_key("*"));
        assert!(!candidate.cooldowns.contains_key("gpt-other"));
        assert!(candidate.enabled);
        assert_eq!(
            request(&base, "gpt-test").await.status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
        *crate::poison::mutex(&server.state.generation_status) = StatusCode::OK;
        *crate::poison::mutex(&server.state.generation_body) = json!({
            "id":"synthetic-response","status":"completed","output":[]
        });
        assert_eq!(request(&base, "gpt-other").await.status(), StatusCode::OK);
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
        task.abort();
    }
}

#[tokio::test]
async fn bps_401_does_not_put_next_request_behind_a_global_authentication_pause() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
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
    let runtime = Arc::new(runtime);
    runtime
        .prepare_basis_points_authorization("account-1")
        .await
        .unwrap();
    let (base, task) = gateway(runtime.clone()).await;
    *crate::poison::mutex(&server.state.generation_status) = StatusCode::UNAUTHORIZED;
    *crate::poison::mutex(&server.state.generation_body) =
        json!({"error":{"code":"token_expired"}});
    assert_eq!(
        request(&base, "gpt-test").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 0);
    assert!(runtime
        .lock_scheduler()
        .candidate("account-1")
        .unwrap()
        .cooldowns
        .is_empty());
    *crate::poison::mutex(&server.state.generation_status) = StatusCode::OK;
    *crate::poison::mutex(&server.state.generation_body) = json!({
        "id":"synthetic-response","status":"completed","output":[]
    });
    let recovered = request(&base, "gpt-test").await;
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
    assert!(runtime.fresh_basis_points_access("account-1").is_some());
    assert_eq!(
        recovered.status(),
        StatusCode::OK,
        "{}",
        recovered.text().await.unwrap()
    );
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
    task.abort();
}

#[tokio::test]
async fn bps_401_does_not_fallback_to_another_eligible_account() {
    let bps = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let native = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    *crate::poison::mutex(&bps.state.generation_status) = StatusCode::UNAUTHORIZED;
    *crate::poison::mutex(&bps.state.generation_body) = json!({"error":{"code":"token_expired"}});
    let mut bps_account = quota_account(QuotaSnapshot::default());
    bps_account.oauth_client_kind = OAuthClientKind::ExcelBps;
    bps_account.priority = 100;
    let mut native_account = quota_account(QuotaSnapshot::default());
    native_account.id = "native-account".into();
    native_account.chatgpt_account_id = "synthetic-native-provider".into();
    native_account.responses_url = native.responses_url.to_string();
    let authority = Arc::new(TokenAuthority::new(2).unwrap());
    for id in ["account-1", "native-account"] {
        authority
            .register(
                id,
                TokenSet::access_only("synthetic-access", None, 1).unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
    }
    let mut runtime = GatewayRuntime::from_mixed_pool(
        Vec::new(),
        vec![bps_account, native_account],
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
    runtime
        .chatgpt_accounts
        .get_mut("account-1")
        .unwrap()
        .basis_points_url = bps.responses_url.clone();
    let runtime = Arc::new(runtime);
    let (base, task) = gateway(runtime.clone()).await;
    assert_eq!(
        request(&base, "gpt-test").await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(bps.state.generation_requests.load(Ordering::SeqCst), 1);
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
    assert!(
        runtime
            .lock_scheduler()
            .candidate("native-account")
            .unwrap()
            .enabled
    );
    task.abort();
}

#[tokio::test]
async fn request_timezone_rewrites_bps_body_and_hot_disable_preserves_configuration() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let runtime = Arc::new(runtime(&server).await);
    let policy = crate::RequestTimezonePolicy {
        enabled: true,
        account_timezones: std::collections::BTreeMap::from([(
            "account-1".into(),
            "Etc/GMT-3".into(),
        )]),
        account_timezone_bindings: std::collections::BTreeMap::new(),
        account_timezone_sources: std::collections::BTreeMap::new(),
    };
    runtime.set_request_timezone(policy.clone()).unwrap();
    let (base, task) = gateway(runtime.clone()).await;
    let environment = "<environment_context><current_date>2026-10-10</current_date><timezone>UTC</timezone></environment_context>";
    for enabled in [true, false] {
        runtime
            .set_request_timezone(crate::RequestTimezonePolicy {
                enabled,
                ..policy.clone()
            })
            .unwrap();
        let response = reqwest::Client::new()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({"model":"gpt-test","input":environment}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
    let bodies = crate::poison::mutex(&server.state.generation_bodies);
    assert_eq!(bodies.len(), 2);
    let user_text = |body: &Value| {
        body["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["role"] == "user")
            .unwrap()["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(
        user_text(&bodies[0]),
        environment.replace("UTC", "Etc/GMT-3")
    );
    assert_eq!(user_text(&bodies[1]), environment);
    task.abort();
}

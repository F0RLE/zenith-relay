use super::*;
use zenith_relay_core::request_timezone::{ProxyTimezone, PROXY_TIMEZONE_MAX_AGE_MS};

fn account_with_timezone(
    id: &str,
    provider: &str,
    upstream: &TestServer,
    priority: i32,
    timezone: Option<&str>,
) -> zenith_relay_core::RuntimeChatGptAccount {
    let mut account = account(id, provider, upstream, priority);
    account.proxy_timezone = timezone.map(|timezone| ProxyTimezone {
        timezone: timezone.into(),
        observed_at_ms: zenith_relay_core::unix_time_ms(),
    });
    account
}

const ENVIRONMENT: &str = "<environment_context><current_date>2026-10-10</current_date><timezone>UTC</timezone></environment_context>";

fn body() -> Value {
    json!({
        "model": MODEL,
        "input": [{"role": "user", "content": ENVIRONMENT}],
        "tools": [{"type": "web_search", "user_location": {
            "type": "approximate", "timezone": "UTC", "city": "Synthetic"
        }}]
    })
}

fn assert_timezone(request: &Value, timezone: &str) {
    let content = &request["input"][0]["content"];
    let text = content
        .as_str()
        .or_else(|| content.get(0)?.get("text")?.as_str())
        .unwrap();
    assert_eq!(text, ENVIRONMENT.replace("UTC", timezone));
    assert_eq!(request["tools"][0]["user_location"]["timezone"], timezone);
    assert_eq!(request["tools"][0]["user_location"]["city"], "Synthetic");
}

#[tokio::test]
async fn request_timezone_http_uses_only_the_selected_proxy_without_a_default() {
    for (configured, expected) in [
        (None, "UTC"),
        (Some("Asia/Tokyo"), "Asia/Tokyo"),
        (Some("Etc/GMT-3"), "Etc/GMT-3"),
    ] {
        let (upstream, state) = spawn_upstream(vec![
            success_reply("synthetic-response"),
            compaction_reply("timezone-compaction"),
        ])
        .await;
        let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
            Vec::new(),
            vec![account_with_timezone(
                "relay-account",
                "synthetic-provider",
                &upstream,
                10,
                configured,
            )],
            vec![mixed_key(None, None)],
            ready_authority("relay-account", "synthetic-access").await,
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
            GatewayRuntimeOptions::default(),
        )
        .await;
        for path in ["/v1/responses", "/v1/responses/compact"] {
            let response = reqwest::Client::new()
                .post(format!("{}{path}", gateway.base_url))
                .bearer_auth(LOCAL_KEY)
                .json(&body())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            assert_timezone(&request.body, expected);
        }
    }
}

#[tokio::test]
async fn request_timezone_does_not_apply_an_expired_proxy_observation() {
    for (fresh, expected) in [(true, "Etc/GMT-3"), (false, "UTC")] {
        let (upstream, state) = spawn_upstream(vec![success_reply("synthetic-response")]).await;
        let mut proxied_account = account("relay-account", "synthetic-provider", &upstream, 10);
        proxied_account.proxy_timezone = Some(ProxyTimezone {
            timezone: "Etc/GMT-3".into(),
            observed_at_ms: zenith_relay_core::unix_time_ms()
                - if fresh { 0 } else { PROXY_TIMEZONE_MAX_AGE_MS },
        });
        let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
            Vec::new(),
            vec![proxied_account],
            vec![mixed_key(None, None)],
            ready_authority("relay-account", "synthetic-access").await,
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
            GatewayRuntimeOptions::default(),
        )
        .await;
        let response = reqwest::Client::new()
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&body())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_timezone(&requests[0].body, expected);
    }
}

#[tokio::test]
async fn request_timezone_does_not_rewrite_api_source_requests() {
    let (upstream, state) = spawn_upstream(vec![success_reply("synthetic-response")]).await;
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
        vec![source(
            "synthetic-source",
            &upstream,
            "synthetic-api-key",
            0,
        )],
        Vec::new(),
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(1).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions::default(),
    )
    .await;
    let original = body();
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&original)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests[0].body["input"], original["input"]);
    assert_eq!(requests[0].body["tools"], original["tools"]);
}

#[tokio::test]
async fn request_timezone_fallback_uses_each_account_without_mutating_client_history() {
    let (first, first_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error":{"code":"rate_limit_exceeded"}}),
    )])
    .await;
    let (second, second_state) = spawn_upstream(vec![success_reply("synthetic-response")]).await;
    let authority = Arc::new(TokenAuthority::new(2).unwrap());
    register_ready(&authority, "first-account", "synthetic-first-access").await;
    register_ready(&authority, "second-account", "synthetic-second-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![
            account_with_timezone(
                "first-account",
                "synthetic-first",
                &first,
                100,
                Some("Etc/GMT-3"),
            ),
            account_with_timezone(
                "second-account",
                "synthetic-second",
                &second,
                10,
                Some("Asia/Tokyo"),
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions::default(),
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&body())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let first_requests = first_state.requests.lock().unwrap();
    let second_requests = second_state.requests.lock().unwrap();
    assert_eq!(first_requests.len(), 1);
    assert_eq!(second_requests.len(), 1);
    assert_timezone(&first_requests[0].body, "Etc/GMT-3");
    assert_timezone(&second_requests[0].body, "Asia/Tokyo");
}

#[tokio::test]
async fn request_timezone_websocket_rewrites_first_and_subsequent_turns() {
    let (upstream, state) = spawn_websocket_upstream().await;
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![account_with_timezone(
            "relay-account",
            "synthetic-provider",
            &upstream,
            10,
            Some("Etc/GMT-3"),
        )],
        vec![mixed_key(None, None)],
        ready_authority("relay-account", "synthetic-access").await,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions::default(),
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
    for _ in 0..2 {
        let mut request = body();
        request["type"] = json!("response.create");
        socket
            .send(ClientWsMessage::Text(request.to_string()))
            .await
            .unwrap();
        receive_websocket_completion(&mut socket).await;
    }
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        assert_timezone(request, "Etc/GMT-3");
    }
}

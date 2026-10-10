use super::*;

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
async fn public_models_group_providers_and_preserve_order_inside_each_group() {
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
            "gpt-image-2",
            "gpt-5.4-mini",
            "gpt-5.6-sol",
            "vendor/claude-haiku-4-5",
            "vendor/gemini-3.6-flash-low",
            "vendor/grok-4.5",
            "private-second",
            "vendor/glm-4.7",
            "vendor/glm-5.2",
            "private-first",
        ]
    );
}

#[tokio::test]
async fn generic_client_keeps_tier_ownership_without_managed_codex_defaults() {
    let (upstream, state) = spawn_upstream("source-key", Vec::new()).await;
    let (gateway, _) = spawn_gateway_with_options(
        vec![source("source", &upstream, "source-key", &[MODEL], 0)],
        vec![local_key("key", LOCAL_KEY, None)],
        GatewayRuntimeOptions {
            model_metadata_catalog: None,
            max_retry_candidates: 3,
            pool_routing: None,
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
            None,
            Some("fast"),
            Some("standard"),
            Some("flex"),
            Some("default"),
            Some("priority"),
        ]
    );
}

#[tokio::test]
async fn codex_catalog_exposes_api_speed_choices_and_honors_explicit_selection() {
    for default_tier in [DefaultServiceTier::Standard, DefaultServiceTier::Ultrafast] {
        let (upstream, state) = spawn_upstream("synthetic-speed-key", Vec::new()).await;
        let (gateway, _) = spawn_gateway_with_options(
            vec![source(
                "source",
                &upstream,
                "synthetic-speed-key",
                &[MODEL],
                0,
            )],
            vec![local_key("key", LOCAL_KEY, None)],
            GatewayRuntimeOptions {
                default_service_tier: default_tier,
                ..GatewayRuntimeOptions::default()
            },
        )
        .await;
        let client = reqwest::Client::new();
        let catalog: Value = client
            .get(format!(
                "{}/v1/models?client_version=0.120.0",
                gateway.base_url
            ))
            .bearer_auth(LOCAL_KEY)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let card = &catalog["models"][0];
        assert_eq!(card["slug"], MODEL);
        assert_eq!(card["service_tiers"][0]["id"], "priority");
        assert_eq!(card["service_tiers"][1]["id"], "ultrafast");
        assert_eq!(card["additional_speed_tiers"], json!(["fast", "ultrafast"]));
        assert!(card.get("default_service_tier").is_none());

        for tier in [
            None,
            Some("priority"),
            Some("fast"),
            Some("ultrafast"),
            Some("default"),
        ] {
            let mut body = json!({"model": MODEL, "input": "synthetic input"});
            if let Some(tier) = tier {
                body["service_tier"] = json!(tier);
            }
            let response = client
                .post(format!("{}/v1/responses", gateway.base_url))
                .bearer_auth(LOCAL_KEY)
                .header("originator", "codex_cli_rs")
                .json(&body)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        let requests = state.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.path == "/v1/models")
                .count(),
            0
        );
        let tiers = requests
            .iter()
            .filter(|request| request.path == "/v1/responses")
            .map(|request| request.body["service_tier"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            tiers,
            [
                (default_tier == DefaultServiceTier::Ultrafast).then_some("ultrafast"),
                Some("priority"),
                Some("fast"),
                Some("ultrafast"),
                Some("default")
            ]
        );
    }
}

#[tokio::test]
async fn explicit_speed_survives_fallback_without_participant_metadata_requests() {
    for tier in ["default", "fast", "priority", "ultrafast", "flex"] {
        let (first, first_state) = spawn_upstream(
            "first-synthetic-key",
            vec![overload_reply("unavailable", None)],
        )
        .await;
        let (second, second_state) = spawn_upstream("second-synthetic-key", Vec::new()).await;
        let (gateway, _) = spawn_gateway_with_options(
            vec![
                source("first", &first, "first-synthetic-key", &[MODEL], 10),
                source("second", &second, "second-synthetic-key", &[MODEL], 0),
            ],
            vec![local_key("key", LOCAL_KEY, None)],
            GatewayRuntimeOptions {
                default_service_tier: DefaultServiceTier::Ultrafast,
                max_retry_candidates: 2,
                ..GatewayRuntimeOptions::default()
            },
        )
        .await;
        let response = reqwest::Client::new()
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model": MODEL, "input": "synthetic input", "service_tier": tier}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        for state in [first_state, second_state] {
            let requests = state.requests.lock().unwrap();
            assert_eq!(requests.len(), 1);
            assert_eq!(requests[0].path, "/v1/responses");
            assert_eq!(requests[0].body["service_tier"], tier);
        }
    }
}

use super::*;

#[tokio::test]
async fn account_catalog_uses_reference_metadata_and_preserves_native_transport() {
    let mut upstream_catalog = default_upstream_model_catalog();
    upstream_catalog["models"][0]["slug"] = Value::String(OFFICIAL_CODEX_MODEL.to_string());
    upstream_catalog["models"][0]["display_name"] = Value::String("GPT-5.6 Terra".to_string());
    // Inventory and transport belong to the account. Its capability fields
    // cannot replace reference semantics or remove an inventoried model.
    upstream_catalog["models"][0]["supported_in_api"] = Value::Bool(false);
    upstream_catalog["models"][0]["default_reasoning_level"] = Value::String("ultra".to_string());
    upstream_catalog["models"][0]["supported_reasoning_levels"] = json!([
        {"effort": "low", "description": "Low"},
        {"effort": "high", "description": "High"},
        {"effort": "xhigh", "description": "Extra high"},
        {"effort": "ultra", "description": "Ultra"}
    ]);
    let (upstream, state) = spawn_upstream_with_catalog(Vec::new(), upstream_catalog).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let mut official_account = account("relay-account", "provider-account", &upstream, 10);
    official_account.models = vec![OFFICIAL_CODEX_MODEL.to_string()];
    let (gateway, events, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![official_account],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions {
            // A saved pool selection narrows the reference modes on every row.
            model_reasoning_allowed_levels: std::collections::BTreeMap::from([(
                OFFICIAL_CODEX_MODEL.to_string(),
                vec!["low".to_string()],
            )]),
            ..reference_metadata_options()
        },
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
        .any(|model| model["slug"] == OFFICIAL_CODEX_MODEL));
    // Native ChatGPT rows retain account transport, with shared model semantics.
    assert!(catalog["models"][0].get("service_tiers").is_some());
    assert_eq!(catalog["models"][0]["use_responses_lite"], true);
    assert_eq!(catalog["models"][0]["supported_in_api"], true);
    assert_eq!(catalog["models"][0]["supports_parallel_tool_calls"], true);
    assert_eq!(catalog["models"][0]["default_reasoning_level"], "low");
    assert_eq!(
        catalog["models"][0]["supported_reasoning_levels"],
        json!([
            {"effort": "low", "description": "low"}
        ])
    );
    assert_eq!(
        catalog["models"][0]["supports_reasoning_summary_parameter"],
        false
    );
    assert_eq!(catalog["models"][0]["supports_reasoning_summaries"], false);
    assert_eq!(catalog["models"][0]["default_reasoning_summary"], "none");
    assert_eq!(
        gateway
            .runtime
            .as_ref()
            .unwrap()
            .model_reasoning_levels(OFFICIAL_CODEX_MODEL),
        ["low", "high", "xhigh"]
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
            "model": OFFICIAL_CODEX_MODEL,
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
        assert_eq!(request.body["model"], OFFICIAL_CODEX_MODEL);
        assert_eq!(
            request.body.get("service_tier").and_then(Value::as_str),
            expected_tier
        );
        assert_eq!(request.responses_lite.as_deref(), Some("true"));
        assert_eq!(request.body["parallel_tool_calls"], false);
        assert_eq!(request.body["reasoning"]["effort"], "xhigh");
        assert_eq!(request.body["reasoning"]["summary"], "detailed");
        assert_eq!(request.body["reasoning"]["context"], "all_turns");
    }
    drop(requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), client_tiers.len());
    assert!(events.iter().all(|event| {
        event.requested_reasoning_effort.as_deref() == Some("xhigh")
            && event.effective_reasoning_effort.as_deref() == Some("xhigh")
    }));
}

#[tokio::test]
async fn pool_catalog_combines_account_inventory_with_reference_semantics() {
    let (first_upstream, first_state) =
        spawn_upstream_with_catalog(Vec::new(), json!({"models": []})).await;
    let (second_upstream, second_state) =
        spawn_upstream_with_catalog(Vec::new(), default_upstream_model_catalog()).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "first-account", "first-access").await;
    register_ready(&authority, "second-account", "second-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &first_upstream, 100),
            account("second-account", "provider-second", &second_upstream, 10),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        reference_metadata_options(),
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
    assert_eq!(
        model["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|level| level["effort"].as_str())
            .collect::<Vec<_>>(),
        ["low", "high", "xhigh"]
    );
    assert!(model.get("service_tiers").is_some());
    assert_eq!(model["supports_reasoning_summaries"], false);
    assert_eq!(first_state.requests.lock().unwrap().len(), 1);
    assert_eq!(second_state.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn pool_catalog_keeps_transport_owned_and_model_semantics_stable_across_accounts() {
    let mut first_catalog = default_upstream_model_catalog();
    first_catalog["models"][0]["display_name"] = Value::String("GPT First Account".into());
    first_catalog["models"][0]["default_reasoning_level"] = Value::String("low".into());
    first_catalog["models"][0]["supported_reasoning_levels"] = json!([
        {"effort": "low", "description": "First low"}
    ]);
    first_catalog["models"][0]["use_responses_lite"] = false.into();
    first_catalog["models"][0]["supports_parallel_tool_calls"] = false.into();

    let mut second_catalog = default_upstream_model_catalog();
    second_catalog["models"][0]["display_name"] = Value::String("GPT Second Account".into());
    second_catalog["models"][0]["default_reasoning_level"] = Value::String("high".into());
    second_catalog["models"][0]["supported_reasoning_levels"] = json!([
        {"effort": "high", "description": "Second high"},
        {"effort": "xhigh", "description": "Second extra high"}
    ]);
    second_catalog["models"][0]["use_responses_lite"] = true.into();
    second_catalog["models"][0]["supports_parallel_tool_calls"] = true.into();

    let (first_upstream, first_state) =
        spawn_upstream_with_catalog(Vec::new(), first_catalog).await;
    let (second_upstream, second_state) =
        spawn_upstream_with_catalog(Vec::new(), second_catalog).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "first-account", "first-access").await;
    register_ready(&authority, "second-account", "second-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &first_upstream, 100),
            account("second-account", "provider-second", &second_upstream, 10),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        reference_metadata_options(),
    )
    .await;
    let url = format!(
        "{}/v1/models?client_version={CODEX_MODELS_CLIENT_VERSION}",
        gateway.base_url
    );
    let client = reqwest::Client::new();

    let first: Value = client
        .get(&url)
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let first_model = first["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["slug"] == MODEL)
        .unwrap();
    assert_eq!(first_model["display_name"], "Reference Model");
    assert_eq!(first_model["default_reasoning_level"], "high");
    assert_eq!(
        first_model["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(first_model["use_responses_lite"], false);
    assert_eq!(first_model["supports_parallel_tool_calls"], true);

    // If the first account is temporarily unreachable, the stale manifest is
    // retained only for that account and cannot overwrite the live second
    // account's native metadata.
    drop(first_upstream);
    let second: Value = client
        .get(&url)
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second_model = second["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["slug"] == MODEL)
        .unwrap();
    assert_eq!(second_model["display_name"], first_model["display_name"]);
    assert_eq!(second_model["default_reasoning_level"], "high");
    assert_eq!(
        second_model["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(second_model["use_responses_lite"], true);
    assert_eq!(second_model["supports_parallel_tool_calls"], true);
    assert_eq!(
        second_model["supported_reasoning_levels"],
        first_model["supported_reasoning_levels"]
    );
    assert_eq!(second_model["service_tiers"], first_model["service_tiers"]);
    assert_eq!(first_state.requests.lock().unwrap().len(), 1);
    assert_eq!(second_state.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn pool_catalog_fetches_account_manifests_concurrently_without_changing_rank() {
    let barrier = Arc::new(Barrier::new(2));
    let first_arrived = Arc::new(AtomicUsize::new(0));
    let mut upstreams = Vec::new();
    for index in 0..2 {
        let barrier = barrier.clone();
        let first_arrived = first_arrived.clone();
        let app = Router::new().route(
            "/v1/models",
            get(move || {
                let barrier = barrier.clone();
                let first_arrived = first_arrived.clone();
                async move {
                    first_arrived.fetch_add(1, Ordering::SeqCst);
                    // A sequential implementation cannot cross this barrier.
                    barrier.wait().await;
                    if index == 0 {
                        tokio::task::yield_now().await;
                    }
                    let mut catalog = default_upstream_model_catalog();
                    catalog["models"][0]["use_responses_lite"] = Value::Bool(index == 0);
                    Json(catalog)
                }
            }),
        );
        upstreams.push(spawn(app).await);
    }
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "account-a", "synthetic-a").await;
    register_ready(&authority, "account-b", "synthetic-b").await;
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![
            account("account-a", "provider-a", &upstreams[0], 100),
            account("account-b", "provider-b", &upstreams[1], 10),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        reference_metadata_options(),
    )
    .await;
    let request = async {
        reqwest::Client::new()
            .get(format!(
                "{}/v1/models?client_version={CODEX_MODELS_CLIENT_VERSION}",
                gateway.base_url
            ))
            .bearer_auth(LOCAL_KEY)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };
    let catalog = tokio::time::timeout(Duration::from_secs(3), request)
        .await
        .unwrap();
    assert_eq!(first_arrived.load(Ordering::SeqCst), 2);
    assert_eq!(catalog["models"][0]["use_responses_lite"], true);
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
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &first_upstream, 100),
            account("second-account", "provider-second", &second_upstream, 10),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        reference_metadata_options(),
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
async fn pool_catalog_ignores_participant_capabilities_on_a_mixed_source_model() {
    let (source_upstream, source_state) = spawn_upstream_with_catalog(
        Vec::new(),
        json!({
            "data": [{
                "id": MODEL,
                "context_length": 1_000_000,
                "input_modalities": ["text", "image"]
            }]
        }),
    )
    .await;
    let mut native_catalog = default_upstream_model_catalog();
    native_catalog["models"][0]["context_window"] = 128_000.into();
    native_catalog["models"][0]["max_context_window"] = 120_000.into();
    native_catalog["models"][0]["input_modalities"] = json!({"invalid":"participant value"});
    native_catalog["models"][0]["supported_reasoning_levels"] = json!("malformed participant enum");
    native_catalog["models"][0]["display_name"] = json!([]);
    let (account_upstream, account_state) =
        spawn_upstream_with_catalog(Vec::new(), native_catalog).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
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
        reference_metadata_options(),
    )
    .await;
    rotation_policy::set_order(&gateway, &["relay-account", "generic-source"]);

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
    assert_eq!(
        model["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|level| level["effort"].as_str())
            .collect::<Vec<_>>(),
        ["low", "high", "xhigh"]
    );
    assert!(model.get("service_tiers").is_some());
    assert_eq!(model["use_responses_lite"], true);
    assert_eq!(model["supports_reasoning_summaries"], false);
    assert_eq!(model["input_modalities"], json!(["text", "image"]));
    assert!(model.get("context_window").is_none());
    assert!(model.get("max_context_window").is_none());

    let source_requests_before = source_state.requests.lock().unwrap().len();
    assert_eq!(source_requests_before, 0);
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
    let mut stale_account = account("stale-account", "stale-provider", &upstream, 10);
    stale_account.models.push("gpt-extra".to_string());
    let ready = account("ready-account", "ready-provider", &upstream, 10);
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![stale_account, ready],
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

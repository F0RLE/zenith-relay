use super::*;

#[test]
fn runtime_updates_service_tier_and_removes_candidates_in_place() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();

    assert_eq!(runtime.default_service_tier(), DefaultServiceTier::Standard);
    runtime.set_default_service_tier(DefaultServiceTier::Fast);
    assert_eq!(runtime.default_service_tier(), DefaultServiceTier::Fast);
    assert!(runtime.remove_candidate("source-1"));
    assert!(runtime.candidate_runtime_order().is_empty());
}
#[test]
fn service_tier_storage_values_keep_fast_aliases_compatible() {
    assert_eq!(DefaultServiceTier::Standard.as_str(), "standard");
    assert_eq!(DefaultServiceTier::Fast.as_str(), "fast");
    assert_eq!(
        DefaultServiceTier::from_storage_value("priority"),
        DefaultServiceTier::Fast
    );
    assert_eq!(
        DefaultServiceTier::from_storage_value("unknown"),
        DefaultServiceTier::Standard
    );
}
#[test]
fn runtime_updates_source_policy_without_rebuilding_candidate_state() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["model-a", "model-b"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let retry_at = current_time_ms() + 60_000;
    runtime.set_candidate_cooldown("source-1", "model-a", retry_at);

    assert!(runtime.update_source_policy(
        "source-1",
        RuntimeCandidatePolicy {
            enabled: true,
            draining: false,
            priority: 7,
            weight: 3,
            allowed_models: vec!["model-b".into()],
            excluded_models: Vec::new(),
        },
        30,
    ));
    assert_eq!(
        runtime.visible_models_for_secret("local-secret", &[WireApi::Responses], current_time_ms()),
        vec!["model-b"]
    );
    let candidate = runtime
        .lock_scheduler()
        .candidate("source-1")
        .cloned()
        .unwrap();
    assert_eq!(candidate.priority, 7);
    assert_eq!(candidate.weight, 3);
    assert_eq!(candidate.cooldowns.get("model-a"), Some(&retry_at));
    assert_eq!(runtime.source_recovery_delay_ms("source-1"), Some(30_000));
}
#[test]
fn runtime_rejects_policy_updates_for_missing_candidates() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["model-a"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let policy = RuntimeCandidatePolicy {
        enabled: true,
        draining: false,
        priority: 7,
        weight: 3,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
    };

    assert!(!runtime.update_source_policies(&[
        RuntimeSourcePolicyUpdate {
            source_id: "source-1".into(),
            policy: policy.clone(),
            recovery_delay_seconds: 30,
        },
        RuntimeSourcePolicyUpdate {
            source_id: "missing".into(),
            policy: policy.clone(),
            recovery_delay_seconds: 30,
        },
    ]));
    assert_eq!(
        runtime
            .lock_scheduler()
            .candidate("source-1")
            .expect("source candidate")
            .priority,
        0
    );
    assert!(!runtime.update_account_policy("missing", policy));
}
#[test]
fn runtime_updates_key_scope_without_rebuild() {
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "a", &["model-a"])),
            RuntimeSource::unrestricted(source("source-b", "b", &["model-b"])),
        ],
        vec![RuntimeLocalKey {
            key: key("key-1", "local-secret"),
            enabled: true,
            source_ids: Some(vec!["source-a".into()]),
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
        }],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();

    assert_eq!(
        runtime.visible_models_for_secret("local-secret", &[WireApi::Responses], current_time_ms()),
        vec!["model-a"]
    );
    assert!(runtime.update_key_scope(
        "key-1",
        CandidateScope {
            source_ids: Some(std::iter::once("source-b".to_string()).collect()),
            account_ids: Some(Default::default()),
            model_rules: ModelRules::default(),
        },
    ));
    assert_eq!(
        runtime.visible_models_for_secret("local-secret", &[WireApi::Responses], current_time_ms()),
        vec!["model-b"]
    );
}
#[test]
fn active_responses_scope_uses_live_candidate_policy() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-a",
            "upstream-secret",
            &["model-a"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let mut account = runtime
        .lock_scheduler()
        .candidate("source-a")
        .cloned()
        .unwrap();
    account.id = "account-a".into();
    account.kind = CandidateKind::OAuthAccount;
    account.source_id = "codex".into();
    account.account_id = Some("account-a".into());
    runtime.lock_scheduler().upsert(account);

    let source_ids = BTreeSet::from(["source-a".to_string()]);
    let account_ids = BTreeSet::from(["account-a".to_string()]);
    assert_eq!(
        runtime.active_responses_scope(&source_ids, &account_ids),
        CandidateScope {
            source_ids: Some(source_ids.clone()),
            account_ids: Some(account_ids.clone()),
            model_rules: ModelRules::default(),
        }
    );

    assert!(runtime.update_source_policy(
        "source-a",
        RuntimeCandidatePolicy {
            enabled: false,
            draining: false,
            priority: 0,
            weight: 1,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
        },
        0,
    ));
    assert!(runtime.update_account_policy(
        "account-a",
        RuntimeCandidatePolicy {
            enabled: false,
            draining: false,
            priority: 0,
            weight: 1,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
        },
    ));
    let scope = runtime.active_responses_scope(&source_ids, &account_ids);
    assert_eq!(scope.source_ids, Some(BTreeSet::new()));
    assert_eq!(scope.account_ids, Some(BTreeSet::new()));
}
#[test]
fn service_tier_normalization_preserves_valid_ids_for_model_policy() {
    let normalized = normalize_model_service_tier_overrides(BTreeMap::from([
        ("provider/gpt-5".to_string(), DefaultServiceTier::Fast),
        ("provider/claude-5".to_string(), DefaultServiceTier::Fast),
    ]))
    .unwrap();

    assert_eq!(
        normalized,
        BTreeMap::from([
            ("provider/gpt-5".to_string(), DefaultServiceTier::Fast),
            ("provider/claude-5".to_string(), DefaultServiceTier::Fast),
        ])
    );
}
#[test]
fn local_auth_returns_only_the_matching_redacted_key_policy() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![
            RuntimeLocalKey::unrestricted(key("key-1", "local-secret")),
            RuntimeLocalKey::unrestricted(key("key-2", "other-secret")),
        ],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();

    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    assert_eq!(authenticated.id, "key-1");
    assert!(runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer upstream-secret")))
        .is_none());
    assert!(!format!("{runtime:?}").contains("local-secret"));
    assert!(!format!("{runtime:?}").contains("upstream-secret"));
}
#[test]
fn explicit_empty_scope_cannot_start_a_gateway() {
    let error = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-a",
            "a",
            &["gpt-a"],
        ))],
        vec![RuntimeLocalKey {
            key: key("key", "secret"),
            enabled: true,
            source_ids: Some(Vec::new()),
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
        }],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("no enabled gateway credential"));
}
#[test]
fn transition_runtime_allows_an_explicitly_empty_scope() {
    let runtime = GatewayRuntime::build(
        vec![RuntimeSource::unrestricted(source(
            "source-a",
            "a",
            &["gpt-a"],
        ))],
        Vec::new(),
        vec![RuntimeMixedLocalKey {
            key: key("key", "secret"),
            enabled: true,
            source_ids: Some(Vec::new()),
            account_ids: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
            wire_apis: None,
        }],
        None,
        ReachabilityRequirement::AllowUnroutable,
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();

    assert!(runtime
        .visible_models_for_secret("secret", &[WireApi::Responses], current_time_ms())
        .is_empty());
}

use super::*;
use crate::providers::chatgpt::{BasisPointsCapturedHeaders, BasisPointsHeader};

#[test]
fn basis_points_headers_match_excel_client_contract() {
    let captured = BasisPointsCapturedHeaders::from_entries(vec![BasisPointsHeader {
        name: "user-agent".to_string(),
        values: vec!["captured-agent/1".to_string()],
    }])
    .unwrap();
    let headers = basis_points_headers("account-1", Some("user-1"), Some(&captured));
    assert_eq!(
        headers
            .get("x-openai-internal-basispoints-client-host")
            .unwrap(),
        "office"
    );
    assert_eq!(
        headers
            .get("x-openai-internal-basispoints-office-host")
            .unwrap(),
        "Excel"
    );
    assert_eq!(
        headers
            .get("x-openai-internal-basispoints-office-platform")
            .unwrap(),
        "PC"
    );
    assert_eq!(headers.get("x-stainless-lang").unwrap(), "js");
    assert_eq!(
        headers.get("x-stainless-package-version").unwrap(),
        "6.31.0"
    );
    assert_eq!(headers.get("x-stainless-retry-count").unwrap(), "0");
    assert_eq!(
        headers.get("x-stainless-runtime").unwrap(),
        "browser:chrome"
    );
    assert_eq!(headers.get("user-agent").unwrap(), "captured-agent/1");
    assert_eq!(headers.get("x-openai-account-user-id").unwrap(), "user-1");
    assert!(headers.get("x-stainless-runtime-version").is_none());
    assert!(headers
        .get("x-openai-internal-basispoints-browser-name")
        .is_none());
    assert!(headers
        .get("x-openai-internal-basispoints-browser-ua-platform")
        .is_none());
    assert!(headers
        .get("x-openai-internal-basispoints-oiiice-host")
        .is_none());
}
#[test]
fn ordinary_accounts_use_native_responses() {
    let oauth = quota_runtime(QuotaSnapshot::default());
    let key = oauth.authenticate_secret("local-secret").unwrap();
    let transport = || {
        oauth
            .executor_route(
                "account-1",
                "gpt-test",
                &key.scope_snapshot(),
                &[WireApi::Responses],
                false,
            )
            .unwrap()
            .account_transport
    };
    assert_eq!(transport(), AccountTransport::NativeResponses);

    let agent = AgentIdentityCredential::new(
        "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g".into(),
        "synthetic-runtime".into(),
        "synthetic-task".into(),
    )
    .unwrap();
    let identity_runtime = quota_runtime_with_agent(QuotaSnapshot::default(), Some(agent));
    let key = identity_runtime
        .authenticate_secret("local-secret")
        .unwrap();
    assert_eq!(
        identity_runtime
            .executor_route(
                "account-1",
                "gpt-test",
                &key.scope_snapshot(),
                &[WireApi::Responses],
                false,
            )
            .unwrap()
            .account_transport,
        AccountTransport::NativeResponses,
    );
}
#[tokio::test]
async fn prepared_oauth_replacement_cannot_dispatch_an_old_lease() {
    use crate::accounts::{AccountAuthState, TokenSet};
    use crate::scheduler::rotation::{DispatchStartError, SharedRequestBudget};

    let runtime = quota_runtime(QuotaSnapshot::default());
    let authority = &runtime.chatgpt_accounts["account-1"].token_authority;
    let first = TokenSet::access_only("synthetic-first", None, 1).unwrap();
    authority
        .register("account-1", first.clone(), AccountAuthState::Active)
        .await
        .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
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
    let prepared = runtime
        .prepare_authorization("account-1", crate::unix_time_ms())
        .await
        .unwrap();

    // Reuse the visible token fields and generation: a credential snapshot is
    // not a slot incarnation. Neither HTTP nor WebSocket may debit a stale send.
    authority
        .register("account-1", first, AccountAuthState::Active)
        .await
        .unwrap();
    assert_eq!(
        lease.begin_rotation_http_dispatch_for(&prepared, &runtime),
        Err(DispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|b| b.wire_attempts()), 0);
    assert!(budget.attempted_members().is_empty());
    assert_eq!(
        lease.begin_rotation_dispatch_for(&prepared, &runtime),
        Err(DispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 0);
}
#[tokio::test]
async fn removed_and_readded_oauth_slot_rejects_pending_http_and_websocket_dispatch() {
    use crate::accounts::{AccountAuthState, TokenSet};
    use crate::scheduler::rotation::{DispatchStartError, SharedRequestBudget};

    let runtime = quota_runtime(QuotaSnapshot::default());
    let authority = &runtime.chatgpt_accounts["account-1"].token_authority;
    let token = TokenSet::access_only("synthetic-access", None, 1).unwrap();
    authority
        .register("account-1", token.clone(), AccountAuthState::Active)
        .await
        .unwrap();
    let prepared = runtime
        .prepare_authorization("account-1", crate::unix_time_ms())
        .await
        .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
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
    assert!(authority.remove("account-1"));
    authority
        .register("account-1", token, AccountAuthState::Active)
        .await
        .unwrap();
    let replacement = runtime
        .prepare_authorization("account-1", crate::unix_time_ms())
        .await
        .unwrap();
    assert_eq!(
        prepared.credential_fingerprint(),
        replacement.credential_fingerprint()
    );
    assert_ne!(prepared.incarnation(), replacement.incarnation());
    assert_eq!(
        lease.begin_rotation_http_dispatch_for(&prepared, &runtime),
        Err(DispatchStartError::CandidateChanged)
    );
    assert_eq!(
        lease.begin_rotation_dispatch_for(&prepared, &runtime),
        Err(DispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|b| b.wire_attempts()), 0);
    assert!(budget.attempted_members().is_empty());
    lease
        .begin_rotation_http_dispatch_for(&replacement, &runtime)
        .unwrap();
    assert_eq!(budget.dispatches(), 1);
    lease.settle_rotation_success(crate::unix_time_ms());
}
#[tokio::test]
async fn agent_task_replacement_and_aba_revoke_prepared_dispatch() {
    use crate::scheduler::rotation::{DispatchStartError, SharedRequestBudget};

    let agent = AgentIdentityCredential::new(
        "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g".into(),
        "synthetic-runtime".into(),
        "synthetic-task".into(),
    )
    .unwrap();
    let runtime = quota_runtime_with_agent(QuotaSnapshot::default(), Some(agent.clone()));
    let prepared = runtime
        .prepare_authorization("account-1", crate::unix_time_ms())
        .await
        .unwrap();
    let account = &runtime.chatgpt_accounts["account-1"];
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
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
    // Mimic the in-runtime task update without calling a real provider.
    for task in ["other-task", "synthetic-task"] {
        let mut identity = account.agent_identity.write().unwrap();
        *identity = Some(agent.with_task_id(task.into()).unwrap());
        account
            .agent_identity_revision
            .fetch_add(1, Ordering::Release);
    }
    let replacement = runtime
        .prepare_authorization("account-1", crate::unix_time_ms())
        .await
        .unwrap();
    assert_eq!(
        prepared.credential_fingerprint(),
        replacement.credential_fingerprint()
    );
    assert_ne!(prepared.incarnation(), replacement.incarnation());
    assert_eq!(
        lease.begin_rotation_http_dispatch_for(&prepared, &runtime),
        Err(DispatchStartError::CandidateChanged)
    );
    assert_eq!(
        lease.begin_rotation_dispatch_for(&prepared, &runtime),
        Err(DispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 0);
    lease
        .begin_rotation_dispatch_for(&replacement, &runtime)
        .unwrap();
    assert_eq!(budget.dispatches(), 1);
    lease.settle_rotation_success(crate::unix_time_ms());
}
#[tokio::test]
async fn model_inventory_refresh_keeps_live_capacity_health_and_exact_dispatch_ownership() {
    use crate::scheduler::rotation::SharedRequestBudget;
    use crate::{PoolMemberKind, PoolRoutingMember, PoolRoutingMode, PoolRoutingPolicy};

    async fn reserve_account(
        runtime: &GatewayRuntime,
        key: &AuthenticatedKey,
        model: &str,
        budget: &SharedRequestBudget,
        now: u64,
    ) -> Option<(crate::Selection, CandidateLease)> {
        runtime
            .select_and_reserve_with_budget(
                key,
                model,
                &[WireApi::Responses],
                &HashSet::new(),
                (None, None),
                now,
                budget,
            )
            .await
    }

    let runtime = quota_runtime(QuotaSnapshot::default());
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let now = crate::unix_time_ms();
    let started_budget = SharedRequestBudget::for_incoming_request(3);
    let (_, started) = reserve_account(&runtime, &key, "gpt-test", &started_budget, now)
        .await
        .unwrap();
    started.begin_rotation_http_dispatch().unwrap();

    // An unstarted lease for the removed route must not become a stale send.
    let pending_budget = SharedRequestBudget::for_incoming_request(3);
    let (_, pending) = reserve_account(&runtime, &key, "gpt-test", &pending_budget, now)
        .await
        .unwrap();
    runtime
        .lock_scheduler()
        .set_pool_routing(PoolRoutingPolicy {
            mode: PoolRoutingMode::InOrder,
            members: vec![PoolRoutingMember {
                kind: PoolMemberKind::Account,
                id: "account-1".into(),
                weight: 1,
                max_concurrency: 1,
            }],
            ..PoolRoutingPolicy::default()
        })
        .unwrap();
    assert!(runtime.update_account_models("account-1", &["gpt-added".into()]));
    assert!(runtime
        .visible_models(&key, &[WireApi::Responses], now)
        .contains(&"gpt-added".to_string()));
    assert_eq!(
        pending.begin_rotation_http_dispatch(),
        Err(crate::scheduler::rotation::DispatchStartError::CandidateChanged)
    );
    assert_eq!(pending_budget.dispatches(), 0);
    drop(pending);

    let added_budget = SharedRequestBudget::for_incoming_request(3);
    assert!(
        tokio::time::timeout(
            Duration::from_millis(25),
            reserve_account(&runtime, &key, "gpt-added", &added_budget, now)
        )
        .await
        .is_err(),
        "new inventory must not bypass the old physical lease"
    );
    assert!(started
        .settle_rotation(
            crate::scheduler::rotation::AttemptObservation {
                execution: crate::scheduler::rotation::ExecutionObservation::committed(),
                health: crate::scheduler::rotation::HealthObservation::Success,
            },
            crate::unix_time_ms(),
        )
        .unwrap()
        .is_some());
    assert!(started
        .settle_rotation(
            crate::scheduler::rotation::AttemptObservation {
                execution: crate::scheduler::rotation::ExecutionObservation::committed(),
                health: crate::scheduler::rotation::HealthObservation::Success,
            },
            crate::unix_time_ms(),
        )
        .unwrap()
        .is_none());
    let (_, added) = reserve_account(&runtime, &key, "gpt-added", &added_budget, now)
        .await
        .unwrap();
    added.begin_rotation_http_dispatch().unwrap();
    added.settle_rotation_success(crate::unix_time_ms());

    assert!(runtime.set_candidate_health("account-1", CandidateHealth::Blocked));
    assert!(runtime.update_account_models("account-1", &["gpt-added".into(), "gpt-new".into()]));
    assert_eq!(
        runtime
            .lock_scheduler()
            .candidate("account-1")
            .unwrap()
            .health,
        CandidateHealth::Blocked,
        "inventory edits are not positive health evidence"
    );
    assert!(reserve_account(
        &runtime,
        &key,
        "gpt-new",
        &SharedRequestBudget::for_incoming_request(3),
        now
    )
    .await
    .is_none());
}
#[test]
fn model_inventory_refresh_updates_image_bridge_and_retires_old_transport_evidence() {
    let runtime = quota_runtime(QuotaSnapshot::default());
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let scope = key.scope_snapshot();
    runtime.remember_codex_model_manifest(
        "account-1",
        serde_json::json!({"models": [{"slug": "gpt-test"}]}),
        1,
    );
    runtime.set_codex_model_uses_responses_lite("account-1", "gpt-test", true);
    assert!(runtime.update_account_models("account-1", &["gpt-5.6-sol".into()]));
    assert!(runtime
        .stale_codex_model_manifests(["account-1"])
        .is_empty());
    assert!(runtime
        .codex_model_responses_lite_candidates("gpt-test")
        .is_empty());
    assert_eq!(
        runtime
            .image_executor_route(
                "account-1",
                IMAGE_API_MODEL,
                &scope,
                &[WireApi::ChatCompletions],
            )
            .unwrap()
            .source_model,
        "gpt-5.6-sol"
    );
    assert!(runtime.update_account_models("account-1", &["gpt-test".into()]));
    assert!(runtime
        .image_executor_route(
            "account-1",
            IMAGE_API_MODEL,
            &scope,
            &[WireApi::ChatCompletions],
        )
        .is_none());
}
#[tokio::test]
async fn removed_and_reintroduced_model_cannot_reuse_an_old_pending_lease() {
    use crate::scheduler::rotation::{DispatchStartError, SharedRequestBudget};

    let runtime = quota_runtime(QuotaSnapshot::default());
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let now = crate::unix_time_ms();
    let budget = SharedRequestBudget::for_incoming_request(3);
    let reserve = |budget: SharedRequestBudget| {
        let runtime = &runtime;
        let key = &key;
        async move {
            runtime
                .select_and_reserve_with_budget(
                    key,
                    "gpt-test",
                    &[WireApi::Responses],
                    &HashSet::new(),
                    (None, None),
                    crate::unix_time_ms(),
                    &budget,
                )
                .await
                .unwrap()
                .1
        }
    };
    let pending = reserve(budget.clone()).await;
    assert!(runtime.update_account_models("account-1", &["gpt-other".into()]));
    assert!(runtime.update_account_models("account-1", &["gpt-test".into()]));
    assert_eq!(
        pending.begin_rotation_http_dispatch(),
        Err(DispatchStartError::CandidateChanged)
    );
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(budget.with_budget(|b| b.wire_attempts()), 0);
    drop(pending);

    let current = reserve(budget.clone()).await;
    current.begin_rotation_http_dispatch().unwrap();
    current.settle_rotation_success(now);
    assert_eq!(budget.dispatches(), 1);
}
#[tokio::test]
async fn changed_image_bridge_revokes_pending_dispatch_without_charging_a_generation() {
    use crate::scheduler::rotation::{DispatchStartError, SharedRequestBudget};

    let runtime = quota_runtime(QuotaSnapshot::default());
    assert!(runtime.update_account_models("account-1", &["gpt-5.4".into(), "gpt-5.6-sol".into()]));
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let scope = key.scope_snapshot();
    let protocols = [WireApi::ChatCompletions];
    let now = crate::unix_time_ms();
    let pending_budget = SharedRequestBudget::for_incoming_request(3);
    let (_, pending) = runtime
        .select_and_reserve_image_with_budget(
            &key,
            IMAGE_API_MODEL,
            &protocols,
            &HashSet::new(),
            now,
            &pending_budget,
        )
        .await
        .unwrap();
    assert_eq!(
        runtime
            .image_executor_route("account-1", IMAGE_API_MODEL, &scope, &protocols)
            .unwrap()
            .source_model,
        "gpt-5.4"
    );
    assert!(runtime.update_account_models("account-1", &["gpt-5.6-sol".into()]));
    assert_eq!(
        pending.begin_rotation_http_dispatch(),
        Err(DispatchStartError::CandidateChanged)
    );
    assert_eq!(pending_budget.dispatches(), 0);
    assert_eq!(
        pending_budget.with_budget(|budget| budget.wire_attempts()),
        0
    );
    drop(pending);

    let fresh_budget = SharedRequestBudget::for_incoming_request(3);
    let (_, fresh) = runtime
        .select_and_reserve_image_with_budget(
            &key,
            IMAGE_API_MODEL,
            &protocols,
            &HashSet::new(),
            now,
            &fresh_budget,
        )
        .await
        .unwrap();
    assert_eq!(
        runtime
            .image_executor_route("account-1", IMAGE_API_MODEL, &scope, &protocols)
            .unwrap()
            .source_model,
        "gpt-5.6-sol"
    );
    fresh.begin_rotation_http_dispatch().unwrap();
    fresh.settle_rotation_success(crate::unix_time_ms());
    assert_eq!(fresh_budget.dispatches(), 1);
}

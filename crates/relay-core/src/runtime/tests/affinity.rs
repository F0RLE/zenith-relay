use super::*;

#[test]
fn response_affinity_persists_and_removes_the_same_scheduler_binding() {
    let store = Arc::new(RecordedResponseAffinityStore::default());
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions {
            response_affinity_store: Some(store.clone()),
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();

    let response_id = "resp-1";
    let affinity_key = runtime.response_affinity_key(Some(response_id)).unwrap();
    runtime.bind_response_affinity(Some(response_id), "source-1", 123);

    assert_eq!(
        *store
            .upserts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![ResponseAffinityBinding {
            key: affinity_key.clone(),
            candidate_id: "source-1".to_string(),
            expires_at_ms: 123 + crate::RESPONSE_AFFINITY_TTL_MS,
        }]
    );
    assert!(runtime.invalidate_response_affinity(Some(&affinity_key)));
    assert!(!runtime.invalidate_response_affinity(Some(&affinity_key)));
    assert_eq!(
        *store
            .deletes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![affinity_key]
    );
}
#[tokio::test]
async fn incomplete_response_affinity_is_connection_scoped_and_never_persisted() {
    let store = Arc::new(RecordedResponseAffinityStore::default());
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-a",
            "secret-a",
            &["gpt-test"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions {
            response_affinity_store: Some(store.clone()),
            ..Default::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let normal_key = runtime.response_affinity_key(Some("resp_partial")).unwrap();
    let connection_key = runtime
        .bind_volatile_response_affinity(Some("resp_partial"), "source-a", "request-1", 123)
        .unwrap();
    assert!(!runtime.has_response_affinity_binding(&normal_key, 123));
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let (selection, lease) = runtime
        .select_and_reserve(
            &authenticated,
            "gpt-test",
            &[WireApi::Responses],
            &HashSet::new(),
            (Some(&connection_key), None),
            124,
        )
        .await
        .unwrap();
    assert!(selection.response_affinity_hit);
    drop(lease);
    assert!(store.upserts.lock().unwrap().is_empty());
    assert!(runtime.invalidate_response_affinity(Some(&connection_key)));
    assert!(!runtime.has_response_affinity_binding(&normal_key, 125));
}
#[tokio::test]
async fn saving_bridge_continuation_binds_its_response_to_the_creating_candidate() {
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "secret-a", &["gpt-test"])),
            RuntimeSource::unrestricted(source("source-b", "secret-b", &["gpt-test"])),
        ],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let bridge_request = crate::prepare_responses_to_messages_scoped(
        &serde_json::json!({"model": "gpt-test", "input": "hello"}),
        "gpt-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
        "source-a",
    )
    .unwrap();
    let bridge_response = crate::protocol::translate_messages_response(
        bridge_request,
        &serde_json::json!({
            "id": "msg-1",
            "stop_reason": "end_turn",
            "content": [{"type": "text", "text": "hello"}]
        }),
    )
    .unwrap();

    runtime.save_messages_bridge_response("key-1", "source-a", &bridge_response, 123);

    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let affinity_key = runtime
        .response_affinity_key(Some(&bridge_response.response_id))
        .unwrap();
    let (selection, lease) = runtime
        .select_and_reserve(
            &authenticated,
            "gpt-test",
            &[WireApi::Responses],
            &HashSet::new(),
            (Some(&affinity_key), None),
            124,
        )
        .await
        .unwrap();

    assert_eq!(selection.candidate_id, "source-a");
    assert!(selection.response_affinity_hit);
    assert_eq!(
        selection.diagnostics.reason,
        crate::SelectionReason::ResponseAffinity
    );
    drop(lease);
}
#[test]
fn prompt_affinity_persists_only_its_opaque_binding_and_ttl() {
    let store = Arc::new(RecordedResponseAffinityStore::default());
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions {
            response_affinity_store: Some(store.clone()),
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();

    runtime.bind_prompt_affinity(Some("cache:opaque-hash"), "source-1", 123);

    assert_eq!(
        *store
            .upserts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![ResponseAffinityBinding {
            key: "cache:opaque-hash".to_string(),
            candidate_id: "source-1".to_string(),
            expires_at_ms: 123 + crate::PROMPT_AFFINITY_TTL_MS,
        }]
    );
}
#[test]
fn prompt_affinity_uses_explicit_cache_key_before_session_context() {
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

    let explicit = runtime.prompt_affinity_key(
        "key-1",
        "gpt-test",
        Some("cache-key"),
        Some("client-session"),
    );
    let session = runtime.prompt_affinity_key("key-1", "gpt-test", None, Some("client-session"));
    let other_session =
        runtime.prompt_affinity_key("key-1", "gpt-test", None, Some("other-session"));

    assert!(explicit.is_some());
    assert!(session.is_some());
    assert_ne!(explicit, session);
    assert_ne!(session, other_session);
    assert_eq!(
        session,
        runtime.prompt_affinity_key("key-1", "gpt-test", None, Some("client-session"))
    );
    assert!(runtime
        .prompt_affinity_key("key-1", "gpt-test", None, None)
        .is_none());
}
#[tokio::test]
async fn selection_restores_persisted_response_affinity_before_reserving() {
    let store = Arc::new(RecordedResponseAffinityStore::default());
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "secret-a", &["gpt-test"])),
            RuntimeSource::unrestricted(source("source-b", "secret-b", &["gpt-test"])),
        ],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions {
            response_affinity_store: Some(store.clone()),
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let response_id = "resp-restored";
    let affinity_key = runtime.response_affinity_key(Some(response_id)).unwrap();
    *store
        .restored_binding
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ResponseAffinityBinding {
        key: affinity_key.clone(),
        candidate_id: "source-b".to_string(),
        expires_at_ms: 123 + crate::RESPONSE_AFFINITY_TTL_MS,
    });
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();

    let (selection, lease) = runtime
        .select_and_reserve(
            &authenticated,
            "gpt-test",
            &[WireApi::Responses],
            &HashSet::new(),
            (Some(&affinity_key), None),
            123,
        )
        .await
        .unwrap();

    assert_eq!(selection.candidate_id, "source-b");
    assert!(selection.response_affinity_hit);
    assert_eq!(
        *store
            .found
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![affinity_key.clone()]
    );
    assert_eq!(
        *store
            .upserts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![ResponseAffinityBinding {
            key: affinity_key,
            candidate_id: "source-b".to_string(),
            expires_at_ms: 123 + crate::RESPONSE_AFFINITY_TTL_MS,
        }]
    );
    drop(lease);
}
#[tokio::test]
async fn selection_restores_persisted_prompt_affinity_before_reserving() {
    let store = Arc::new(RecordedResponseAffinityStore::default());
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "secret-a", &["gpt-test"])),
            RuntimeSource::unrestricted(source("source-b", "secret-b", &["gpt-test"])),
        ],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions {
            response_affinity_store: Some(store.clone()),
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    assert!(runtime.update_candidate_availability_at(
        "source-a",
        true,
        CandidateHealth::Healthy,
        CandidateQuota::Available(6_500),
        Some(123),
    ));
    assert!(runtime.update_candidate_availability_at(
        "source-b",
        true,
        CandidateHealth::Healthy,
        CandidateQuota::Available(9_000),
        Some(123),
    ));
    let affinity_key = "cache:restored-prompt";
    *store
        .restored_binding
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ResponseAffinityBinding {
        key: affinity_key.to_string(),
        candidate_id: "source-a".to_string(),
        expires_at_ms: 123 + crate::PROMPT_AFFINITY_TTL_MS,
    });
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();

    let (selection, lease) = runtime
        .select_and_reserve(
            &authenticated,
            "gpt-test",
            &[WireApi::Responses],
            &HashSet::new(),
            (None, Some(affinity_key)),
            123,
        )
        .await
        .unwrap();

    assert_eq!(selection.candidate_id, "source-a");
    assert_eq!(
        selection.diagnostics.reason,
        crate::SelectionReason::PromptCacheAffinity
    );
    assert_eq!(
        *store
            .found
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![affinity_key.to_string()]
    );
    drop(lease);
}
#[test]
fn response_affinity_owner_tracks_live_key_scope() {
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "a", &["gpt-test"])),
            RuntimeSource::unrestricted(source("source-b", "b", &["gpt-test"])),
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
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    runtime.bind_response_affinity(Some("resp-1"), "source-a", 1);
    let affinity_key = runtime.response_affinity_key(Some("resp-1")).unwrap();

    assert_eq!(
        runtime.response_affinity_owner_supports_route(
            &authenticated,
            &affinity_key,
            "gpt-test",
            &[WireApi::Responses],
            2,
        ),
        Some(true)
    );
    assert!(runtime.update_key_scope(
        "key-1",
        CandidateScope {
            source_ids: Some(BTreeSet::from(["source-b".to_string()])),
            ..CandidateScope::default()
        },
    ));
    assert_eq!(
        runtime.response_affinity_owner_supports_route(
            &authenticated,
            &affinity_key,
            "gpt-test",
            &[WireApi::Responses],
            2,
        ),
        Some(false),
        "removing a provider from the key scope must release its chat affinity"
    );
}
#[test]
fn optional_response_affinity_is_released_when_owner_needs_reauthentication() {
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "a", &["gpt-test"])),
            RuntimeSource::unrestricted(source("source-b", "b", &["gpt-test"])),
        ],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    runtime.bind_response_affinity(Some("resp-1"), "source-a", 1);
    let affinity_key = runtime.response_affinity_key(Some("resp-1")).unwrap();
    assert!(runtime.set_candidate_health("source-a", CandidateHealth::ReauthRequired));
    assert_eq!(
        runtime.response_affinity_owner_supports_route(
            &authenticated,
            &affinity_key,
            "gpt-test",
            &[WireApi::Responses],
            2,
        ),
        Some(true)
    );
    assert_eq!(
        runtime.response_affinity_owner_is_eligible(
            &authenticated,
            &affinity_key,
            "gpt-test",
            &[WireApi::Responses],
            2,
        ),
        Some(false)
    );

    let mut optional_affinity = Some(affinity_key.clone());
    assert!(runtime.release_unroutable_response_affinity(
        &authenticated,
        &mut optional_affinity,
        "gpt-test",
        &[WireApi::Responses],
        2,
    ));
    assert!(optional_affinity.is_none());
    assert!(runtime.has_response_affinity_binding(&affinity_key, 2));
    assert_eq!(
        runtime
            .response_affinity_candidate(&affinity_key, 2)
            .as_deref(),
        Some("source-a")
    );
}

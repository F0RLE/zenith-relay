use super::*;
use crate::{
    GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeLocalKey, RuntimeSource, WireApi,
};
use reqwest::header::HeaderMap;
use std::sync::Arc;

fn runtime(recovery_delay_seconds: u64) -> GatewayRuntime {
    let mut source = RuntimeSource::unrestricted(ProviderSource {
        id: "source".into(),
        name: "source".into(),
        base_url: "https://example.test/v1".into(),
        api_key: "test-source-key".into(),
        wire_api: WireApi::Responses,
        models: vec!["model-a".into(), "model-b".into()],
        proxy_url: None,
    });
    source.recovery_delay_seconds = recovery_delay_seconds;
    GatewayRuntime::from_pool(
        vec![source],
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "key".into(),
            secret: "test-local-key".into(),
        })],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap()
}

async fn reserve(runtime: &GatewayRuntime) -> crate::runtime::CandidateLease {
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer test-local-key")))
        .unwrap();
    runtime
        .select_and_reserve(
            &key,
            "model-a",
            &[WireApi::Responses],
            &Default::default(),
            (None, None),
            now_ms(),
        )
        .await
        .unwrap()
        .1
}

#[tokio::test]
async fn public_model_aliases_share_the_canonical_route_cooldown() {
    for prefix in [None, Some("team")] {
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: "source".into(),
                name: "source".into(),
                base_url: "https://example.test/v1".into(),
                api_key: "synthetic-source-key".into(),
                wire_api: WireApi::Responses,
                models: vec!["vendor/model".into(), "vendor/model-mini".into()],
                proxy_url: None,
            })],
            vec![RuntimeLocalKey {
                model_prefix: prefix.map(str::to_owned),
                ..RuntimeLocalKey::unrestricted(LocalGatewayKey {
                    id: "key".into(),
                    secret: "synthetic-local-key".into(),
                })
            }],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static(
                "Bearer synthetic-local-key",
            )))
            .unwrap();
        let public = prefix.map_or_else(
            || "vendor/model".to_owned(),
            |prefix| format!("{prefix}/vendor/model"),
        );
        let model = runtime
            .resolve_visible_model(&key, &public, &[WireApi::Responses], now_ms())
            .unwrap();
        let (_, lease) = runtime
            .select_and_reserve(
                &key,
                &model,
                &[WireApi::Responses],
                &Default::default(),
                (None, None),
                now_ms(),
            )
            .await
            .unwrap();
        let route = runtime
            .executor_route(
                lease.candidate_id(),
                &model,
                &key.scope_snapshot(),
                &[WireApi::Responses],
                false,
            )
            .unwrap();
        assert_eq!(route.source_model, "vendor/model");
        let mut headers = HeaderMap::new();
        headers.insert(RETRY_AFTER, HeaderValue::from_static("120"));
        let failure = AttemptFailure::classified_with_hint(
            StatusCode::TOO_MANY_REQUESTS,
            error_codes::UPSTREAM_RATE_LIMITED,
            Default::default(),
        );
        let state = settle_route_failure(&runtime, &lease, &route, &failure, &headers);
        assert_eq!(state.cooldown_scope.as_deref(), Some("vendor/model"));
        for alias in [
            public.clone(),
            public.to_uppercase(),
            crate::codex_model_alias(&public),
            crate::claude_desktop_model_alias(&public),
            crate::claude_desktop_reasoning_model_alias(&public),
            crate::claude_desktop_catalog_model_alias(&public),
        ] {
            let model = runtime
                .resolve_configured_model(&key, &alias, &[WireApi::Responses])
                .unwrap();
            assert_eq!(crate::model_id_key(&model), "vendor/model");
            assert!(
                runtime
                    .select_and_reserve(
                        &key,
                        &model,
                        &[WireApi::Responses],
                        &Default::default(),
                        (None, None),
                        now_ms()
                    )
                    .await
                    .is_none(),
                "cooldown bypass through {alias}"
            );
        }
        assert!(
            runtime
                .select_and_reserve(
                    &key,
                    "vendor/model-mini",
                    &[WireApi::Responses],
                    &Default::default(),
                    (None, None),
                    now_ms()
                )
                .await
                .is_some(),
            "similar names are not equivalent quota identities"
        );
    }
}

#[tokio::test]
async fn same_candidate_wait_requires_a_temporary_proven_rejection_and_opt_in() {
    use crate::scheduler::rotation::ExecutionObservation;
    for enabled in [false, true] {
        for (status, category, temporary) in [
            (429, error_codes::UPSTREAM_RATE_LIMITED, true),
            (503, error_codes::UPSTREAM_OVERLOADED, true),
            (503, error_codes::UPSTREAM_MODEL_CAPACITY, true),
            (429, error_codes::UPSTREAM_QUOTA_EXHAUSTED, false),
            (401, error_codes::UPSTREAM_UNAUTHORIZED, false),
            (400, error_codes::UPSTREAM_INVALID_REQUEST, false),
        ] {
            for (execution, rejected) in [
                (ExecutionObservation::not_sent(), true),
                (ExecutionObservation::unknown(), false),
                (ExecutionObservation::committed(), false),
            ] {
                let runtime = runtime(0);
                runtime.set_route_recovery_enabled(enabled);
                let lease = reserve(&runtime).await;
                lease.begin_rotation_http_dispatch().unwrap();
                let mut failure = AttemptFailure::classified_with_hint(
                    StatusCode::from_u16(status).unwrap(),
                    category,
                    Default::default(),
                );
                failure.execution = execution;
                settle_attempt_failure(&runtime, &lease, "model-a", &failure, &HeaderMap::new());
                assert_eq!(
                    lease.has_transient_retry(),
                    enabled && temporary && rejected,
                    "{category}"
                );
            }
        }
    }
}

#[tokio::test]
async fn repeated_retry_fields_cannot_shorten_the_selected_model_cooldown() {
    for values in [["1", "120"], ["120", "1"]] {
        let runtime = runtime(5);
        let mut headers = reqwest::header::HeaderMap::new();
        for value in values {
            headers.append(RETRY_AFTER, value.parse().unwrap());
        }
        let started = now_ms();
        let lease = reserve(&runtime).await;
        let failure = settle_classified_failure(
            &runtime,
            &lease,
            "model-a",
            StatusCode::SERVICE_UNAVAILABLE,
            "upstream_overloaded",
            &headers,
            RateLimitBodyHint::default(),
        );
        assert!(failure.retry_at_ms.unwrap() >= started + 120_000);
    }
}

#[tokio::test]
async fn model_failure_preserves_provider_retry_after_over_shorter_source_delay() {
    for category in [
        "upstream_overloaded",
        "upstream_model_capacity",
        "upstream_model_not_found",
        "upstream_model_unsupported",
        "upstream_candidate_rejected",
    ] {
        for (header, body) in [
            (Some(120), None),
            (None, Some(120_000)),
            (Some(10), Some(120_000)),
        ] {
            let runtime = runtime(5);
            let mut headers = reqwest::header::HeaderMap::new();
            if let Some(seconds) = header {
                headers.insert(RETRY_AFTER, seconds.to_string().parse().unwrap());
            }
            let started = now_ms();
            let lease = reserve(&runtime).await;
            let failure = settle_classified_failure(
                &runtime,
                &lease,
                "model-a",
                StatusCode::SERVICE_UNAVAILABLE,
                category,
                &headers,
                RateLimitBodyHint {
                    retry_clock: body.map(|delay| {
                        crate::scheduler::ProviderClock::new(started + delay, Some(started))
                    }),
                    global: false,
                    ..Default::default()
                },
            );
            let retry_at = failure.retry_at_ms.unwrap();
            assert!(
                retry_at >= started + 120_000,
                "{category} returned too early"
            );
            assert!(
                retry_at <= now_ms() + 120_000,
                "{category} delayed too long"
            );
            assert_eq!(failure.cooldown_scope.as_deref(), Some("model-a"));
            let snapshot = runtime.candidate_runtime_order();
            assert!(snapshot[0].available, "the other model must remain usable");
            assert_eq!(snapshot[0].model_retries[0].retry_at_ms, retry_at);
        }
    }
}

#[test]
fn delayed_failure_settlement_does_not_restart_the_provider_reset_clock() {
    let runtime = runtime(5);
    let observed = UNIX_EPOCH + Duration::from_millis(1_700_000_000_900);
    let now = observed + Duration::from_secs(5);
    let headers = reqwest::header::HeaderMap::new();
    for payload in [
        serde_json::json!({"error":{"resets_at":1_700_000_010_900_u64}}),
        serde_json::json!({"error":{"resets_in_seconds":10}}),
        serde_json::json!({"error":{"retry_after":10}}),
    ] {
        let result = failure_cooldown(CooldownInput {
            runtime: &runtime,
            candidate_id: "source",
            model: "model-a",
            status: StatusCode::TOO_MANY_REQUESTS,
            category: error_codes::UPSTREAM_RATE_LIMITED,
            headers: &headers,
            hint: cooldown::rate_limit_body_hint_value(&payload, observed),
            now,
        })
        .unwrap();
        let clock = result.provider_clock.unwrap();
        assert_eq!(clock.not_before_ms, 1_700_000_010_900);
        assert_eq!(clock.observed_at_ms, Some(1_700_000_000_900));
        assert_eq!(result.retry_at_ms, 1_700_000_010_900);
    }
}

#[test]
fn subsecond_provider_reset_remains_separate_from_the_local_recovery_floor() {
    let runtime = runtime(5);
    let now = UNIX_EPOCH + Duration::from_millis(1_700_000_000_900);
    let headers = reqwest::header::HeaderMap::new();
    for reset in [
        serde_json::json!(1_700_000_000_950_u64),
        serde_json::json!("1700000000950"),
    ] {
        let payload = serde_json::json!({"error":{"resets_at":reset}});
        let result = failure_cooldown(CooldownInput {
            runtime: &runtime,
            candidate_id: "source",
            model: "model-a",
            status: StatusCode::TOO_MANY_REQUESTS,
            category: error_codes::UPSTREAM_RATE_LIMITED,
            headers: &headers,
            hint: cooldown::rate_limit_body_hint_value(&payload, now),
            now,
        })
        .unwrap();
        assert_eq!(result.scope, "model-a");
        assert_eq!(
            result.provider_clock.unwrap().not_before_ms,
            1_700_000_000_950
        );
        assert_eq!(result.retry_at_ms, 1_700_000_005_900);
    }
}

#[tokio::test]
async fn settlement_retains_clock_provenance_without_renewing_expired_evidence() {
    for known_observation in [false, true] {
        for seconds in [1, 60] {
            let runtime = runtime(5);
            let lease = reserve(&runtime).await;
            let observed_ms = now_ms();
            let observed = UNIX_EPOCH + Duration::from_millis(observed_ms);
            let settled_ms = observed_ms + 5_000;
            let mut hint = cooldown::rate_limit_body_hint_value(
                &serde_json::json!({"retry_after":seconds}),
                observed,
            );
            if !known_observation {
                hint.retry_clock.as_mut().unwrap().observed_at_ms = None;
            }
            let failure = AttemptFailure::classified_with_hint(
                StatusCode::TOO_MANY_REQUESTS,
                error_codes::UPSTREAM_RATE_LIMITED,
                hint,
            );
            let headers = reqwest::header::HeaderMap::new();
            let cooldown = failure_cooldown(CooldownInput {
                runtime: &runtime,
                candidate_id: "source",
                model: "model-a",
                status: failure.status,
                category: failure.category,
                headers: &headers,
                hint,
                now: UNIX_EPOCH + Duration::from_millis(settled_ms),
            });
            failure.settle_rotation_rejection(&runtime, &lease, cooldown, settled_ms);
            let (_, effective, provider) =
                runtime.failure_state_for("source", "model-a", settled_ms);
            if seconds == 1 {
                assert!(provider.is_none());
                assert_eq!(effective, Some(("model-a".into(), settled_ms + 5_000)));
            } else {
                let provider = provider.unwrap();
                assert_eq!(provider.not_before_ms, observed_ms + 60_000);
                assert_eq!(
                    provider.observed_at_ms,
                    known_observation.then_some(observed_ms)
                );
                assert_eq!(effective, Some(("model-a".into(), observed_ms + 60_000)));
            }
        }
    }
}

#[test]
fn relayed_subscription_reset_does_not_cool_the_entire_api_key() {
    let runtime = runtime(5);
    let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let empty_headers = reqwest::header::HeaderMap::new();
    for (code, explicit, header, expected_ms) in [
        ("usage_limit_reached", None, None, 5_000),
        (
            "subscription_sharing_usage_limit_exceeded",
            Some(7),
            None,
            7_000,
        ),
        ("usage_limit_reached", Some(7), Some("120"), 120_000),
    ] {
        let body = serde_json::to_vec(&serde_json::json!({"error": {
            "type":code, "plan_type":"plus", "resets_at":1_700_518_400_u64,
            "retry_after":explicit
        }}))
        .unwrap();
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(value) = header {
            headers.insert(RETRY_AFTER, HeaderValue::from_static(value));
        }
        let hint =
            cooldown::rate_limit_body_hint_at(&body, now).with_retry_headers_at(&headers, now);
        let failure = AttemptFailure::classified_with_hint(
            StatusCode::TOO_MANY_REQUESTS,
            error_codes::UPSTREAM_QUOTA_EXHAUSTED,
            hint,
        );
        assert_eq!(
            failure
                .cooldown_hint
                .retry_clock_for_account(true)
                .unwrap()
                .not_before_ms,
            1_700_518_400_000,
        );
        // Streaming paths may retain retry headers only in the failure hint.
        let result = failure_cooldown(CooldownInput {
            runtime: &runtime,
            candidate_id: "source",
            model: "model-a",
            status: failure.status,
            category: failure.category,
            headers: &empty_headers,
            hint: failure.cooldown_hint,
            now,
        })
        .unwrap();
        assert_eq!(result.scope, "model-a");
        assert_eq!(result.retry_at_ms, 1_700_000_000_000 + expected_ms);
        let provider_delay = explicit
            .map(|seconds| seconds * 1_000)
            .into_iter()
            .chain(header.map(|seconds| seconds.parse::<u64>().unwrap() * 1_000))
            .max();
        assert_eq!(
            result.provider_clock.map(|clock| clock.not_before_ms),
            provider_delay.map(|delay| 1_700_000_000_000 + delay)
        );
    }
    let hint = cooldown::rate_limit_body_hint_at(
        br#"{"error":{"code":"insufficient_quota","resets_at":1700518400}}"#,
        now,
    );
    let result = failure_cooldown(CooldownInput {
        runtime: &runtime,
        candidate_id: "source",
        model: "model-a",
        status: StatusCode::TOO_MANY_REQUESTS,
        category: error_codes::UPSTREAM_QUOTA_EXHAUSTED,
        headers: &empty_headers,
        hint,
        now,
    })
    .unwrap();
    assert_eq!(result.scope, "*");
    assert_eq!(result.retry_at_ms, 1_700_518_400_000);
    assert_eq!(
        result.provider_clock.unwrap().not_before_ms,
        1_700_518_400_000
    );
}

#[test]
fn relayed_subscription_reset_does_not_become_an_http_retry_after_header() {
    let now = SystemTime::now();
    let body = serde_json::to_vec(&serde_json::json!({"error":{
        "type":"usage_limit_reached",
        "resets_at":crate::unix_time_ms_at(now) + 518_400_000,
    }}))
    .unwrap();
    let hint = cooldown::rate_limit_body_hint_at(&body, now);
    for (origin, expected) in [
        (crate::ErrorOrigin::Provider, None),
        (crate::ErrorOrigin::Account, Some("518400")),
    ] {
        let failure = AttemptFailure::classified_with_hint(
            StatusCode::TOO_MANY_REQUESTS,
            error_codes::UPSTREAM_QUOTA_EXHAUSTED,
            hint,
        );
        let response = crate::gateway::execution::attempt_error_response(
            failure,
            None,
            origin,
            "synthetic-request",
        );
        assert_eq!(
            response
                .headers()
                .get(RETRY_AFTER)
                .map(|value| value.to_str().unwrap()),
            expected
        );
    }
}

#[tokio::test]
async fn generic_failure_preserves_provider_retry_after() {
    let runtime = runtime(5);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(RETRY_AFTER, HeaderValue::from_static("120"));
    let started = now_ms();
    let lease = reserve(&runtime).await;
    let failure = settle_classified_failure(
        &runtime,
        &lease,
        "model-a",
        StatusCode::SERVICE_UNAVAILABLE,
        "upstream_status",
        &headers,
        RateLimitBodyHint::default(),
    );
    assert!(failure.retry_at_ms.unwrap() >= started + 120_000);
    assert_eq!(failure.cooldown_scope.as_deref(), Some("*"));
}

#[tokio::test]
async fn model_access_403_only_cools_the_requested_model() {
    for (category, expected_scope) in [
        (error_codes::UPSTREAM_MODEL_UNAVAILABLE, Some("model-a")),
        (error_codes::UPSTREAM_FORBIDDEN, Some("*")),
        (error_codes::UPSTREAM_CONTENT_POLICY, None),
    ] {
        let runtime = runtime(5);
        let lease = reserve(&runtime).await;
        let state = settle_classified_failure(
            &runtime,
            &lease,
            "model-a",
            StatusCode::FORBIDDEN,
            category,
            &reqwest::header::HeaderMap::new(),
            RateLimitBodyHint::default(),
        );
        assert_eq!(state.cooldown_scope.as_deref(), expected_scope);
        assert_eq!(
            current_failure_state(&runtime, "source", "model-b")
                .retry_at_ms
                .is_some(),
            expected_scope == Some("*")
        );
    }
}

#[tokio::test]
async fn global_retry_scope_survives_an_equal_circuit_deadline_after_dispatch() {
    let runtime = runtime(5);
    let lease = reserve(&runtime).await;
    lease.begin_rotation_dispatch().unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(RETRY_AFTER, HeaderValue::from_static("120"));
    let state = settle_classified_failure(
        &runtime,
        &lease,
        "model-a",
        StatusCode::INTERNAL_SERVER_ERROR,
        error_codes::UPSTREAM_SERVER_ERROR,
        &headers,
        RateLimitBodyHint::default(),
    );
    assert_eq!(state.consecutive_failures, 1);
    assert_eq!(state.cooldown_scope.as_deref(), Some("*"));
    assert_eq!(
        current_failure_state(&runtime, "source", "model-b").retry_at_ms,
        state.retry_at_ms
    );
}

#[tokio::test]
async fn model_failures_without_retry_hints_keep_their_recovery_policy() {
    for (category, mandatory) in [
        ("upstream_overloaded", false),
        ("upstream_candidate_rejected", true),
        ("upstream_stream", false),
        ("stream_incomplete", false),
        ("stream_idle_timeout", false),
        ("upstream_model_capacity", true),
        ("upstream_model_not_found", true),
        ("upstream_model_unsupported", true),
    ] {
        let runtime = runtime(5);
        let started = now_ms();
        let lease = reserve(&runtime).await;
        let failure = settle_classified_failure(
            &runtime,
            &lease,
            "model-a",
            StatusCode::SERVICE_UNAVAILABLE,
            category,
            &reqwest::header::HeaderMap::new(),
            RateLimitBodyHint::default(),
        );
        assert_eq!(failure.retry_at_ms.is_some(), mandatory, "{category}");
        if let Some(retry_at) = failure.retry_at_ms {
            assert!(retry_at >= started + 5_000);
            assert!(retry_at <= now_ms() + 5_000);
            assert_eq!(failure.cooldown_scope.as_deref(), Some("model-a"));
        }
    }
}

#[tokio::test]
async fn degraded_route_pauses_the_whole_account() {
    let runtime = runtime(5);
    let started = now_ms();
    let lease = reserve(&runtime).await;
    let failure = settle_classified_failure(
        &runtime,
        &lease,
        "model-a",
        StatusCode::NOT_FOUND,
        error_codes::UPSTREAM_ROUTE_DEGRADED,
        &reqwest::header::HeaderMap::new(),
        RateLimitBodyHint::default(),
    );
    let retry_at = failure.retry_at_ms.expect("degraded route cooldown");
    assert!(retry_at >= started + 30 * 60_000);
    assert_eq!(failure.cooldown_scope.as_deref(), Some("*"));
    assert_eq!(
        current_failure_state(&runtime, "source", "model-b").retry_at_ms,
        Some(retry_at)
    );
}

#[tokio::test]
async fn disabled_degraded_route_policy_does_not_pause_the_account() {
    let runtime = runtime(5);
    runtime.set_block_degraded_routes_enabled(false);
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer test-local-key")))
        .unwrap();
    assert!(
        runtime
            .resolve_model(&key, "gpt-6-astra-degrade2-luna-1p")
            .is_some()
    );
    runtime.set_block_degraded_routes_enabled(true);
    assert!(
        runtime
            .resolve_model(&key, "gpt-6-astra-degrade2-luna-1p")
            .is_none()
    );
    runtime.set_block_degraded_routes_enabled(false);

    let lease = reserve(&runtime).await;
    let failure = settle_classified_failure(
        &runtime,
        &lease,
        "model-a",
        StatusCode::NOT_FOUND,
        error_codes::UPSTREAM_ROUTE_DEGRADED,
        &reqwest::header::HeaderMap::new(),
        RateLimitBodyHint::default(),
    );
    assert_eq!(failure.cooldown_scope.as_deref(), Some("model-a"));
    assert_eq!(
        current_failure_state(&runtime, "source", "model-b").retry_at_ms,
        None
    );
}

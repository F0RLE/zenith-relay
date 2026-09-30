use super::*;
use crate::{
    GatewayRuntimeOptions, LocalGatewayKey, ProviderSource, RuntimeLocalKey, RuntimeSource, WireApi,
};
use std::sync::Arc;

fn runtime(recovery_delay_seconds: u64) -> GatewayRuntime {
    let mut source = RuntimeSource::unrestricted(ProviderSource {
        id: "source".into(),
        name: "source".into(),
        base_url: "https://example.test/v1".into(),
        api_key: "test-source-key".into(),
        wire_api: WireApi::Responses,
        models: vec!["model-a".into(), "model-b".into()],
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
                    retry_after_ms: body,
                    global: false,
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

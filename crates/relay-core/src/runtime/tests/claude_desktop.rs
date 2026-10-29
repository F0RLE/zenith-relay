use super::*;

fn desktop_aliases(model: &str) -> [String; 4] {
    use sha2::{Digest, Sha256};
    let legacy_suffix = Sha256::digest(crate::model_id_key(model).as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("-");
    [
        crate::claude_desktop_model_alias(model),
        crate::claude_desktop_reasoning_model_alias(model),
        format!("claude-zenith-{legacy_suffix}"),
        format!("claude-fable-5-zenith-{legacy_suffix}"),
    ]
}

fn desktop_runtime(models: &[&str]) -> GatewayRuntime {
    GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source",
            "synthetic-upstream",
            models,
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap()
}

fn assert_resolution(runtime: &GatewayRuntime, requested: &str, expected: Option<&str>) {
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    assert_eq!(
        runtime
            .resolve_visible_model(
                &authenticated,
                requested,
                &[WireApi::Messages],
                current_time_ms(),
            )
            .as_deref(),
        expected,
        "visible model: {requested}"
    );
    assert_eq!(
        runtime
            .resolve_configured_model(&authenticated, requested, &[WireApi::Messages])
            .as_deref(),
        expected,
        "configured model: {requested}"
    );
}

#[test]
fn claude_desktop_aliases_resolve_real_models_without_changing_other_client_ids() {
    let runtime = desktop_runtime(&["gpt-test", "vendor/model"]);
    for model in ["gpt-test", "vendor/model"] {
        for alias in desktop_aliases(model) {
            assert_resolution(&runtime, &alias, Some(model));
            assert_resolution(
                &runtime,
                &format!(" {} ", alias.to_uppercase()),
                Some(model),
            );
            for effort in ["low", "medium", "high", "xhigh", "max", "ultra"] {
                assert_resolution(&runtime, &format!("{alias}-effort-{effort}"), None);
            }
        }
        assert_resolution(&runtime, model, Some(model));
        assert_resolution(&runtime, &crate::codex_model_alias(model), Some(model));
    }
    for alias in desktop_aliases("unknown") {
        assert_resolution(&runtime, &alias, None);
    }
    assert_resolution(&runtime, "claude-zenith-invalid", None);
    assert_resolution(&runtime, "claude-fable-5-zenith-invalid", None);
    assert_resolution(&runtime, "claude-sonnet-5-relay-invalid", None);
    assert_resolution(&runtime, "claude-fable-5-relay-invalid", None);
    assert_resolution(&runtime, "unknown", None);
}

#[test]
fn claude_desktop_aliases_respect_hidden_models() {
    let runtime = desktop_runtime(&["gpt-test"]);
    runtime.set_hidden_models(vec!["GPT-TEST".into()]);
    for alias in desktop_aliases("gpt-test") {
        assert_resolution(&runtime, &alias, None);
    }
    assert_resolution(&runtime, "gpt-test", None);
}

#[test]
fn claude_desktop_aliases_preserve_key_scope_and_model_rules() {
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source(
                "source",
                "synthetic-upstream",
                &["gpt-allowed", "gpt-denied", "other-model"],
            )),
            RuntimeSource::unrestricted(source("outside", "synthetic-other", &["gpt-outside"])),
        ],
        vec![RuntimeLocalKey {
            source_ids: Some(vec!["source".into()]),
            allowed_models: vec!["gpt-*".into()],
            excluded_models: vec!["gpt-denied".into()],
            model_prefix: Some("team".into()),
            ..RuntimeLocalKey::unrestricted(key("key", "local-secret"))
        }],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    for alias in desktop_aliases("team/gpt-allowed") {
        assert_resolution(&runtime, &alias, Some("gpt-allowed"));
    }
    for model in [
        "gpt-allowed",
        "team/gpt-denied",
        "team/other-model",
        "team/gpt-outside",
        "other/gpt-allowed",
    ] {
        for alias in desktop_aliases(model) {
            assert_resolution(&runtime, &alias, None);
        }
    }
}

#[test]
fn claude_desktop_aliases_do_not_shadow_or_admit_raw_reserved_names() {
    for alias in desktop_aliases("gpt-test") {
        let runtime = desktop_runtime(&[
            &alias,
            "gpt-test",
            "claude-zenith-unmanaged",
            "claude-fable-5-zenith-unmanaged",
            "claude-sonnet-5-relay-unmanaged",
            "claude-fable-5-relay-unmanaged",
        ]);
        assert_resolution(&runtime, &alias, Some("gpt-test"));
        for nested_alias in desktop_aliases(&alias) {
            assert_resolution(&runtime, &nested_alias, Some(&alias));
        }
        assert_resolution(&runtime, "claude-zenith-unmanaged", None);
        assert_resolution(&runtime, "claude-fable-5-zenith-unmanaged", None);
        assert_resolution(&runtime, "claude-sonnet-5-relay-unmanaged", None);
        assert_resolution(&runtime, "claude-fable-5-relay-unmanaged", None);

        let without_original = desktop_runtime(&[&alias]);
        assert_resolution(&without_original, &alias, None);
    }
}

#[test]
fn claude_desktop_aliases_do_not_depend_on_pool_order_or_duplicate_sources() {
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source(
                "second",
                "synthetic-second",
                &["vendor/second", "vendor/first"],
            )),
            RuntimeSource::unrestricted(source(
                "first",
                "synthetic-first",
                &["vendor/first", "vendor/second"],
            )),
        ],
        vec![RuntimeLocalKey::unrestricted(key("key", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    for model in ["vendor/first", "vendor/second"] {
        for alias in desktop_aliases(model) {
            assert_resolution(&runtime, &alias, Some(model));
        }
    }
}

#[test]
fn claude_desktop_aliases_keep_configured_routes_during_temporary_health_outage() {
    let runtime = desktop_runtime(&["gpt-test"]);
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    assert!(runtime.set_candidate_health("source", CandidateHealth::Unhealthy));
    for alias in desktop_aliases("gpt-test") {
        assert!(
            runtime
                .resolve_visible_model(
                    &authenticated,
                    &alias,
                    &[WireApi::Messages],
                    current_time_ms(),
                )
                .is_none()
        );
        assert_eq!(
            runtime
                .resolve_configured_model(&authenticated, &alias, &[WireApi::Messages])
                .as_deref(),
            Some("gpt-test")
        );
    }
}

#[test]
fn claude_desktop_aliases_resolve_account_routes_without_changing_account_scope() {
    let runtime = quota_runtime(QuotaSnapshot::default());
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    for alias in desktop_aliases("gpt-test") {
        assert_eq!(
            runtime
                .resolve_configured_account_model(&authenticated, &alias)
                .as_deref(),
            Some("gpt-test")
        );
    }
    runtime.update_key_scope(
        "key-1",
        CandidateScope {
            account_ids: Some(BTreeSet::new()),
            ..CandidateScope::default()
        },
    );
    for alias in desktop_aliases("gpt-test") {
        assert!(
            runtime
                .resolve_configured_account_model(&authenticated, &alias)
                .is_none()
        );
    }
}

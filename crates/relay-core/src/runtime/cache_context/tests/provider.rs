use super::*;

fn baseline(store: &CacheContextStore, request: &Value) {
    let observed = begin(store, request, "first", 10);
    let diagnostics = store.prepare(&observed, request, "candidate-a", 11);
    let mut event = event("first", "candidate-a", diagnostics, true);
    event.observe_response_metadata(&json!({
        "id":"resp_synthetic_cache", "status":"completed", "model":"synthetic",
        "prompt_cache_diagnostics":{"type":"cache_miss","reason":"input_changed",
            "cache_missed_tokens":2048,"comparison_reusable_tokens":4096,
            "private":"Synthetic content must not survive"}
    }));
    let cache = event
        .routing
        .as_ref()
        .unwrap()
        .cache_context
        .as_ref()
        .unwrap();
    let encoded = serde_json::to_string(cache).unwrap();
    assert!(encoded.contains("input_changed"));
    assert!(!encoded.contains("resp_synthetic_cache"));
    assert!(!encoded.contains("Synthetic content"));
    store.finish(&mut event, 12);
}

#[test]
fn provider_comparison_preserves_cache_controls_and_never_loads_history() {
    for options in [
        None,
        Some(json!({"mode":"explicit","ttl":"30m"})),
        Some(json!({"comparison_response_id":"resp_client"})),
        Some(json!({"comparison_response_id":null})),
        Some(Value::Null),
    ] {
        let store = CacheContextStore::default();
        let request = payload();
        baseline(&store, &request);
        let mut next = request.clone();
        if let Some(options) = options {
            next["prompt_cache_options"] = options;
        }
        let original = next.clone();
        let observed = begin(&store, &next, "second", 20);
        let diagnostics = store.prepare(&observed, &next, "candidate-a", 21);
        add_provider_comparison(&observed, &mut next, &diagnostics, 21);
        assert_eq!(next["input"], original["input"]);
        assert_eq!(next["reasoning"], original["reasoning"]);
        assert_eq!(next["tools"], original["tools"]);
        assert!(next.get("previous_response_id").is_none());
        assert!(next.get("prompt_cache_key").is_none());
        if original
            .get("prompt_cache_options")
            .is_some_and(|value| value.is_null() || value.get("comparison_response_id").is_some())
        {
            assert_eq!(next, original);
        } else {
            assert_eq!(
                next["prompt_cache_options"]["comparison_response_id"],
                "resp_synthetic_cache"
            );
            assert_eq!(
                next["prompt_cache_options"]["mode"],
                original["prompt_cache_options"]["mode"]
            );
            assert_eq!(
                next["prompt_cache_options"]["ttl"],
                original["prompt_cache_options"]["ttl"]
            );
        }
    }
}

#[test]
fn provider_comparison_rejects_foreign_expired_or_overlapping_baselines() {
    for case in ["candidate", "model", "expired", "overlapping"] {
        let store = CacheContextStore::default();
        let mut request = payload();
        baseline(&store, &request);
        if case == "model" {
            request["model"] = json!("different-model");
        }
        let at = if case == "expired" {
            BASELINE_TTL_MS + 20
        } else {
            20
        };
        let observed = begin(&store, &request, "second", at);
        let _overlap = (case == "overlapping").then(|| begin(&store, &request, "third", at));
        let candidate = if case == "candidate" {
            "candidate-b"
        } else {
            "candidate-a"
        };
        let diagnostics = store.prepare(&observed, &request, candidate, at);
        add_provider_comparison(&observed, &mut request, &diagnostics, at);
        assert!(request.get("prompt_cache_options").is_none(), "{case}");
    }
}

#[test]
fn unknown_provider_metadata_cannot_poison_a_comparison_or_usage() {
    let store = CacheContextStore::default();
    let request = payload();
    let observed = begin(&store, &request, "first", 10);
    let diagnostics = store.prepare(&observed, &request, "candidate-a", 11);
    let mut event = event("first", "candidate-a", diagnostics, false);
    for response in [
        json!({"id":"resp_in_progress","status":"in_progress"}),
        json!({"id":"private content", "status":"completed",
            "prompt_cache_diagnostics":{"type":"cache_miss","reason":"private text","cache_missed_tokens":100}}),
        json!({"id":"x".repeat(257), "status":"completed"}),
    ] {
        event.observe_response_metadata(&response);
    }
    let cache = event
        .routing
        .as_ref()
        .unwrap()
        .cache_context
        .as_ref()
        .unwrap();
    assert!(cache.provider.is_none());
    assert!(cache.provider_response_id.is_none());
    assert!(event.cached_input_tokens.is_none());
    assert!(event.cache_write_input_tokens.is_none());
    event.observe_response_metadata(&json!({"id":"resp_failed", "status":"completed"}));
    store.finish(&mut event, 12);
    let mut next = request;
    let observed = begin(&store, &next, "second", 20);
    let diagnostics = store.prepare(&observed, &next, "candidate-a", 21);
    add_provider_comparison(&observed, &mut next, &diagnostics, 21);
    assert!(next.get("prompt_cache_options").is_none());
}

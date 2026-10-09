use super::*;
use crate::usage::{CacheContextSection, CacheHistoryComparison, CacheInputKind};
use crate::{DefaultServiceTier, RoutingDiagnostics, SelectionReason, ToolUseDiagnostics};
use serde_json::json;

fn payload() -> Value {
    json!({
        "model": "synthetic",
        "instructions": "Synthetic stable instructions.",
        "reasoning": {"effort": "high"},
        "tools": [
            {"type": "function", "name": "synthetic_a", "parameters": {"type": "object"}},
            {"type": "function", "name": "synthetic_b", "parameters": {"type": "object"}}
        ],
        "input": [
            {"role": "developer", "content": "Synthetic context."},
            {"role": "user", "content": "Synthetic turn."}
        ]
    })
}

fn begin(store: &CacheContextStore, request: &Value, id: &str, at: u64) -> CacheContextObservation {
    store.begin(request, "synthetic-key", id, Some("synthetic-session"), at)
}

fn event(
    id: &str,
    candidate: &str,
    diagnostics: CacheContextDiagnostics,
    success: bool,
) -> UsageEvent {
    UsageEvent {
        request_id: id.into(),
        attempt: 1,
        local_key_id: "synthetic-key".into(),
        source_id: "synthetic-source".into(),
        candidate_id: Some(candidate.into()),
        account_id: None,
        account_token_generation: None,
        client_context_id: None,
        routing: Some(RoutingDiagnostics {
            reason: SelectionReason::OnlyEligible,
            eligible_candidates: 1,
            quota_remaining_basis_points: None,
            in_flight_before: 0,
            dispatches_before: 0,
            endpoint_kind: Some("responses".into()),
            cache_context: Some(diagnostics),
        }),
        requested_model: Some("synthetic".into()),
        resolved_model: Some("synthetic".into()),
        requested_reasoning_effort: None,
        effective_reasoning_effort: None,
        wire_api: WireApi::Responses,
        transport: crate::UsageTransport::Http,
        service_tier: DefaultServiceTier::Standard,
        applied_service_tier: None,
        success,
        http_status: if success { 200 } else { 502 },
        error_category: None,
        tool_use: ToolUseDiagnostics::default(),
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: None,
        latency_ms: 1,
        ttft_ms: None,
        generation_ms: None,
        input_tokens: None,
        cached_input_tokens: None,
        cache_write_input_tokens: None,
        cache_write_ttl: None,
        reasoning_tokens: None,
        output_tokens: None,
        total_tokens: None,
        upstream_error: None,
        quota_snapshot: None,
    }
}

fn complete(store: &CacheContextStore, request: &Value, id: &str, at: u64) {
    let observed = begin(store, request, id, at);
    let diagnostics = store.prepare(&observed, request, "candidate-a", at);
    let mut event = event(id, "candidate-a", diagnostics, true);
    store.finish(&mut event, at + 1);
}

#[test]
fn compares_client_upstream_and_relay_separately() {
    let store = CacheContextStore::default();
    let first = payload();
    let observed = begin(&store, &first, "first", 10);
    let diagnostics = store.prepare(&observed, &first, "candidate-a", 11);
    assert_eq!(diagnostics.baseline, CacheContextBaseline::FirstObservation);
    assert_eq!(
        diagnostics.relay_history.comparison,
        CacheHistoryComparison::Unchanged
    );
    store.finish(&mut event("first", "candidate-a", diagnostics, true), 12);

    let mut next = first.clone();
    next["tools"].as_array_mut().unwrap().reverse();
    next["input"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role": "user", "content": "Synthetic append."}));
    let mut upstream = next.clone();
    upstream["reasoning"]["effort"] = json!("low");
    let observed = begin(&store, &next, "second", 20);
    let diagnostics = store.prepare(&observed, &upstream, "candidate-b", 21);
    assert_eq!(diagnostics.baseline, CacheContextBaseline::CompletedRequest);
    assert_eq!(diagnostics.client_changes, vec![CacheContextSection::Tools]);
    assert_eq!(
        diagnostics.upstream_changes,
        vec![CacheContextSection::Tools, CacheContextSection::Reasoning]
    );
    assert_eq!(
        diagnostics.relay_changes,
        vec![CacheContextSection::Reasoning]
    );
    assert_eq!(
        diagnostics.client_history.comparison,
        CacheHistoryComparison::Appended
    );
    assert_eq!(diagnostics.client_history.shared_prefix_items, Some(2));
    assert_eq!(
        diagnostics.upstream_history.comparison,
        CacheHistoryComparison::Appended
    );
    assert_eq!(diagnostics.candidate_changed, Some(true));
    assert_eq!(diagnostics.previous_completed_age_ms, Some(9));
}

#[test]
fn classifies_rewrites_truncation_and_item_kind_without_retaining_content() {
    let store = CacheContextStore::default();
    let first = payload();
    complete(&store, &first, "first", 10);
    let mut rewritten = first.clone();
    rewritten["input"][0]["content"] = json!("SYNTHETIC_MUST_NOT_ESCAPE");
    let observed = begin(&store, &rewritten, "rewrite", 20);
    let diagnostics = store.prepare(&observed, &rewritten, "candidate-a", 21);
    assert_eq!(
        diagnostics.client_history.comparison,
        CacheHistoryComparison::Rewritten
    );
    assert_eq!(diagnostics.client_history.shared_prefix_items, Some(0));
    assert_eq!(
        diagnostics.client_history.first_changed_item_kind,
        Some(CacheInputKind::Developer)
    );
    let encoded = serde_json::to_string(&diagnostics).unwrap();
    for private in [
        "SYNTHETIC_MUST_NOT_ESCAPE",
        "synthetic_a",
        "synthetic-session",
        "synthetic-key",
        "digest",
        "salt",
    ] {
        assert!(!encoded.contains(private), "must not serialize {private}");
    }
    drop(observed);
    let mut truncated = first.clone();
    truncated["input"].as_array_mut().unwrap().pop();
    let observed = begin(&store, &truncated, "truncate", 30);
    let diagnostics = store.prepare(&observed, &truncated, "candidate-a", 31);
    assert_eq!(
        diagnostics.client_history.comparison,
        CacheHistoryComparison::Truncated
    );
    assert_eq!(diagnostics.client_history.shared_prefix_items, Some(1));
}

#[test]
fn continuations_never_compare_deltas_as_complete_histories() {
    let store = CacheContextStore::default();
    let first = payload();
    complete(&store, &first, "first", 10);
    let mut delta = first.clone();
    delta["previous_response_id"] = json!("resp_synthetic");
    delta["input"] = json!([{"role": "user", "content": "Synthetic delta."}]);
    let observed = begin(&store, &delta, "delta", 20);
    let diagnostics = store.prepare(&observed, &delta, "candidate-a", 21);
    assert_eq!(
        diagnostics.client_history.comparison,
        CacheHistoryComparison::Continuation
    );
    assert_eq!(diagnostics.client_history.shared_prefix_items, None);
    assert_eq!(
        diagnostics.relay_history.comparison,
        CacheHistoryComparison::Unchanged
    );
    store.finish(&mut event("delta", "candidate-a", diagnostics, true), 22);
    let observed = begin(&store, &first, "full", 30);
    assert_eq!(
        store
            .prepare(&observed, &first, "candidate-a", 31)
            .client_history
            .comparison,
        CacheHistoryComparison::NotCompared
    );
}

#[test]
fn reports_materialized_continuation_as_relay_history_change() {
    let store = CacheContextStore::default();
    let mut client = payload();
    client["previous_response_id"] = json!("resp_synthetic");
    client["input"] = json!([{"role": "user", "content": "Synthetic delta."}]);
    let observed = begin(&store, &client, "delta", 10);
    let diagnostics = store.prepare(&observed, &payload(), "candidate-a", 11);
    assert_eq!(
        diagnostics.client_history.comparison,
        CacheHistoryComparison::Continuation
    );
    assert_eq!(
        diagnostics.relay_history.comparison,
        CacheHistoryComparison::Rewritten
    );
    assert_eq!(
        diagnostics.relay_history.first_changed_item_kind,
        Some(CacheInputKind::Developer)
    );
}

#[test]
fn failure_wrong_candidate_and_retry_do_not_replace_baseline() {
    let store = CacheContextStore::default();
    let first = payload();
    complete(&store, &first, "first", 10);
    let mut retry = first.clone();
    retry["instructions"] = json!("Synthetic attempt.");
    let observed = begin(&store, &retry, "retry", 20);
    let diagnostics = store.prepare(&observed, &retry, "candidate-a", 21);
    store.finish(
        &mut event("retry", "candidate-b", diagnostics.clone(), true),
        22,
    );
    store.finish(&mut event("retry", "candidate-a", diagnostics, false), 23);
    let diagnostics = store.prepare(&observed, &first, "candidate-b", 24);
    assert_eq!(
        diagnostics.client_changes,
        vec![CacheContextSection::Instructions]
    );
    assert!(diagnostics.upstream_changes.is_empty());
    store.finish(&mut event("retry", "candidate-b", diagnostics, true), 25);
    let observed = begin(&store, &retry, "next", 30);
    let diagnostics = store.prepare(&observed, &first, "candidate-b", 31);
    assert!(diagnostics.client_changes.is_empty());
    assert_eq!(diagnostics.candidate_changed, Some(false));
}

#[test]
fn overlapping_requests_are_corrected_at_completion_and_never_become_baseline() {
    for reverse in [false, true] {
        let store = CacheContextStore::default();
        let first = payload();
        complete(&store, &first, "first", 10);
        let observed_a = begin(&store, &first, "overlap-a", 20);
        let diagnostics_a = store.prepare(&observed_a, &first, "candidate-a", 21);
        assert_eq!(
            diagnostics_a.baseline,
            CacheContextBaseline::CompletedRequest
        );
        let mut changed = first.clone();
        changed["tools"] = json!([]);
        let observed_b = begin(&store, &changed, "overlap-b", 22);
        let diagnostics_b = store.prepare(&observed_b, &changed, "candidate-b", 23);
        let mut events = [
            event("overlap-a", "candidate-a", diagnostics_a, true),
            event("overlap-b", "candidate-b", diagnostics_b, true),
        ];
        if reverse {
            events.reverse();
        }
        for event in &mut events {
            store.finish(event, 24);
            let diagnostics = event
                .routing
                .as_ref()
                .unwrap()
                .cache_context
                .as_ref()
                .unwrap();
            assert_eq!(
                diagnostics.baseline,
                CacheContextBaseline::OverlappingRequests
            );
            assert!(diagnostics.client_changes.is_empty());
            assert_eq!(diagnostics.candidate_changed, None);
            assert_eq!(diagnostics.client_history.shared_prefix_items, None);
        }
        let observed = begin(&store, &first, "next", 30);
        let diagnostics = store.prepare(&observed, &first, "candidate-a", 31);
        assert_eq!(diagnostics.baseline, CacheContextBaseline::CompletedRequest);
        assert!(diagnostics.upstream_changes.is_empty());
        assert_eq!(diagnostics.previous_completed_age_ms, Some(20));
    }
}

#[test]
fn session_and_authenticated_key_boundaries_are_isolated() {
    let store = CacheContextStore::default();
    let request = payload();
    complete(&store, &request, "first", 10);
    for (key, session) in [
        ("other-key", "synthetic-session"),
        ("synthetic-key", "other-session"),
    ] {
        let observed = store.begin(&request, key, "other", Some(session), 20);
        let diagnostics = store.prepare(&observed, &request, "candidate-a", 21);
        assert_eq!(diagnostics.baseline, CacheContextBaseline::FirstObservation);
    }
}

#[test]
fn cache_key_is_a_marked_fallback_and_missing_scope_is_unavailable() {
    let store = CacheContextStore::default();
    let mut request = payload();
    let observed = store.begin(&request, "synthetic-key", "none", None, 10);
    assert_eq!(
        store
            .prepare(&observed, &request, "candidate-a", 11)
            .baseline,
        CacheContextBaseline::Unavailable
    );
    request["prompt_cache_key"] = json!("synthetic-cache-key");
    let observed = store.begin(&request, "synthetic-key", "cache", None, 20);
    let diagnostics = store.prepare(&observed, &request, "candidate-a", 21);
    assert_eq!(diagnostics.scope, CacheContextScope::CacheKey);
    store.finish(&mut event("cache", "candidate-a", diagnostics, true), 22);
    let observed = store.begin(&request, "synthetic-key", "next", None, 30);
    assert_eq!(
        store
            .prepare(&observed, &request, "candidate-a", 31)
            .baseline,
        CacheContextBaseline::CompletedRequest
    );
}

#[test]
fn ttl_and_drop_release_idle_and_cancelled_state() {
    let store = CacheContextStore::default();
    let request = payload();
    complete(&store, &request, "first", 10);
    let abandoned = begin(&store, &request, "abandoned", 20);
    drop(abandoned);
    assert!(crate::poison::mutex(&store.inner.state).active.is_empty());
    let observed = begin(&store, &request, "after-ttl", BASELINE_TTL_MS + 11);
    assert_eq!(
        store
            .prepare(&observed, &request, "candidate-a", BASELINE_TTL_MS + 12)
            .baseline,
        CacheContextBaseline::FirstObservation
    );
}

#[test]
fn diagnostic_state_is_bounded_without_evicting_live_observations() {
    let store = CacheContextStore::default();
    let request = payload();
    let mut held = Vec::new();
    for index in 0..MAX_ACTIVE_REQUESTS {
        held.push(store.begin(
            &request,
            "synthetic-key",
            &format!("req-{index}"),
            Some(&format!("session-{index}")),
            10,
        ));
    }
    let excess = begin(&store, &request, "excess", 11);
    assert_eq!(
        store.prepare(&excess, &request, "candidate-a", 12).baseline,
        CacheContextBaseline::Unavailable
    );
    assert_eq!(
        crate::poison::mutex(&store.inner.state).active.len(),
        MAX_ACTIVE_REQUESTS
    );
    drop(held);
    let observed = begin(&store, &request, "after-release", 13);
    assert_eq!(
        store
            .prepare(&observed, &request, "candidate-a", 14)
            .baseline,
        CacheContextBaseline::FirstObservation
    );
    drop(observed);
    for index in 0..MAX_SCOPES * 2 {
        let _observed = store.begin(
            &request,
            "synthetic-key",
            &format!("idle-{index}"),
            Some(&format!("idle-session-{index}")),
            20,
        );
    }
    assert_eq!(
        crate::poison::mutex(&store.inner.state).scope_count(),
        MAX_SCOPES
    );
}

#[test]
fn random_failure_disables_comparison_instead_of_using_a_weak_salt() {
    let store = CacheContextStore {
        inner: Arc::new(StoreInner {
            salt: None,
            state: Mutex::default(),
        }),
    };
    let observed = begin(&store, &payload(), "first", 10);
    assert_eq!(
        store
            .prepare(&observed, &payload(), "candidate-a", 11)
            .baseline,
        CacheContextBaseline::Unavailable
    );
    assert!(crate::poison::mutex(&store.inner.state).active.is_empty());
}

#[test]
fn cache_comparison_response_id_is_excluded_but_actual_controls_are_compared() {
    let salt = [1; 32];
    let first = RequestFingerprint::capture(&payload(), &salt).unwrap();
    let mut next = payload();
    next["prompt_cache_options"] = json!({"comparison_response_id": "resp_synthetic"});
    assert!(RequestFingerprint::capture(&next, &salt)
        .unwrap()
        .changes_from(&first)
        .is_empty());
    next["prompt_cache_options"]["ttl"] = json!("30m");
    assert_eq!(
        RequestFingerprint::capture(&next, &salt)
            .unwrap()
            .changes_from(&first),
        vec![CacheContextSection::CachePolicy]
    );
}

#[test]
fn capture_bounds_cover_schema_and_input_work_and_counts_are_json_bytes() {
    let store = CacheContextStore::default();
    let mut oversized = payload();
    oversized["instructions"] = json!("x".repeat(8 * 1024 * 1024));
    let observed = begin(&store, &oversized, "large-instructions", 10);
    assert_eq!(
        store
            .prepare(&observed, &payload(), "candidate-a", 11)
            .baseline,
        CacheContextBaseline::SizeLimit
    );
    drop(observed);
    oversized = payload();
    oversized["input"] = json!(vec![json!({"role": "user", "content": "synthetic"}); 1_025]);
    let observed = begin(&store, &oversized, "many-items", 20);
    assert_eq!(
        store
            .prepare(&observed, &payload(), "candidate-a", 21)
            .baseline,
        CacheContextBaseline::SizeLimit
    );
    drop(observed);
    let request = payload();
    let observed = begin(&store, &request, "normal", 30);
    let diagnostics = store.prepare(&observed, &request, "candidate-a", 31);
    assert_eq!(
        diagnostics.client_history.input_bytes,
        Some(serde_json::to_vec(&request["input"]).unwrap().len() as u64)
    );
    let other = CacheContextStore::default();
    let other_observed = begin(&other, &request, "normal", 30);
    assert_ne!(
        observed.inner.client.as_ref().unwrap().sections,
        other_observed.inner.client.as_ref().unwrap().sections
    );
}

#[test]
fn routing_json_accepts_legacy_rows_and_round_trips_safe_comparisons() {
    let store = CacheContextStore::default();
    let request = payload();
    let observed = begin(&store, &request, "synthetic", 10);
    let diagnostics = store.prepare(&observed, &request, "candidate-a", 11);
    let routing = event("synthetic", "candidate-a", diagnostics, true)
        .routing
        .unwrap();
    let mut encoded = serde_json::to_value(&routing).unwrap();
    assert!(encoded["cacheContext"].is_object());
    assert_eq!(
        serde_json::from_value::<RoutingDiagnostics>(encoded.clone()).unwrap(),
        routing
    );

    encoded.as_object_mut().unwrap().remove("cacheContext");
    let legacy = serde_json::from_value::<RoutingDiagnostics>(encoded).unwrap();
    assert!(legacy.cache_context.is_none());
    assert!(serde_json::to_value(legacy)
        .unwrap()
        .get("cacheContext")
        .is_none());
}

use super::*;

fn prepend(client: &Value) -> Value {
    let mut upstream = client.clone();
    upstream["input"].as_array_mut().unwrap().insert(
        0,
        json!({"role": "developer", "content": "SYNTHETIC_PRIVATE_RELAY_CONTEXT"}),
    );
    upstream
}

#[test]
fn only_the_relay_owned_catalog_prefix_is_excluded_from_alignment() {
    let store = CacheContextStore::default();
    let mut client = payload();
    client["input"].as_array_mut().unwrap().extend([
        json!({"type":"additional_tools","role":"developer","tools":[]}),
        json!({"role":"user","content":"Synthetic next."}),
    ]);
    let mut upstream = prepend(&client);
    upstream["input"].as_array_mut().unwrap().insert(
        0,
        json!({"type":"additional_tools","role":"developer","tools":[]}),
    );
    let observed = begin(&store, &client, "synthetic", 10);
    let unproven = store.prepare(&observed, &upstream, "candidate-a", 11);
    let aligned =
        |upstream: &Value| store.prepare_with_prefix(&observed, upstream, "candidate-a", 11, 0..2);
    let proven = aligned(&upstream);
    assert_eq!(unproven.relay_history.shared_prefix_items, Some(0));
    assert_eq!(proven.relay_history.shared_prefix_items, Some(0));
    assert_eq!(
        unproven
            .relay_history
            .leading_context_alignment
            .unwrap()
            .comparison,
        CacheHistoryComparison::Rewritten
    );
    let alignment = proven.relay_history.leading_context_alignment.unwrap();
    assert_eq!(alignment.skipped_client_items, 1);
    assert_eq!(alignment.skipped_upstream_items, 3);
    assert_eq!(alignment.skipped_upstream_relay_items, Some(2));
    assert_eq!(alignment.shared_prefix_items, 3);
    assert_eq!(alignment.comparison, CacheHistoryComparison::Unchanged);
    upstream["input"][4]["tools"] = json!([{"type":"function","name":"synthetic"}]);
    let edited = aligned(&upstream)
        .relay_history
        .leading_context_alignment
        .unwrap();
    assert_eq!(edited.shared_prefix_items, 1);
    assert_eq!(edited.comparison, CacheHistoryComparison::Rewritten);
}

#[test]
fn bps_alignment_counts_only_the_generated_developer_wrapper() {
    let store = CacheContextStore::default();
    let client = json!({"model":"synthetic","input":[{"role":"user","content":"question"}]});
    let upstream = json!({"model":"synthetic","input":[
        {"role":"developer","content":"client instructions"},
        {"role":"developer","content":"synthetic BPS wrapper"},
        {"role":"user","content":"question"}
    ]});
    let observed = begin(&store, &client, "first", 10);
    let diagnostics = store.prepare_with_prefix(&observed, &upstream, "candidate-a", 11, 1..2);
    let alignment = diagnostics.relay_history.leading_context_alignment.unwrap();
    assert_eq!(alignment.skipped_upstream_items, 2);
    assert_eq!(alignment.skipped_upstream_relay_items, Some(1));
    assert_eq!(alignment.comparison, CacheHistoryComparison::Unchanged);
    assert_eq!(
        diagnostics.relay_history.comparison,
        CacheHistoryComparison::Rewritten
    );
}

#[test]
fn relay_alignment_keeps_literal_and_temporal_prefixes_distinct() {
    let store = CacheContextStore::default();
    let client = payload();
    let upstream = prepend(&client);
    let observed = begin(&store, &client, "first", 10);
    let first = store.prepare(&observed, &upstream, "candidate-a", 11);
    store.finish(&mut event("first", "candidate-a", first, true), 12);

    let mut client = client;
    client["input"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role": "user", "content": "Next."}));
    let upstream = prepend(&client);
    let observed = begin(&store, &client, "second", 20);
    let diagnostics = store.prepare(&observed, &upstream, "candidate-a", 21);
    assert_eq!(diagnostics.client_history.shared_prefix_items, Some(2));
    assert_eq!(diagnostics.upstream_history.shared_prefix_items, Some(3));
    assert_eq!(diagnostics.relay_history.shared_prefix_items, Some(0));
    assert_eq!(
        diagnostics.relay_history.comparison,
        CacheHistoryComparison::Rewritten
    );
    let alignment = diagnostics
        .relay_history
        .leading_context_alignment
        .as_ref()
        .unwrap();
    assert_eq!(alignment.skipped_client_items, 1);
    assert_eq!(alignment.skipped_upstream_items, 2);
    assert_eq!(alignment.shared_prefix_items, 2);
    assert_eq!(alignment.comparison, CacheHistoryComparison::Unchanged);
    let encoded = serde_json::to_string(&diagnostics).unwrap();
    assert!(!encoded.contains("SYNTHETIC_PRIVATE"));
    assert!(!encoded.contains("digest"));
    assert!(!encoded.contains("salt"));
}

#[test]
fn relay_alignment_does_not_hide_body_rewrites_or_continuation_materialization() {
    let store = CacheContextStore::default();
    let client = payload();
    let mut upstream = prepend(&client);
    upstream["input"][2]["content"] = json!("Changed body.");
    let observed = begin(&store, &client, "rewrite", 10);
    let diagnostics = store.prepare(&observed, &upstream, "candidate-a", 11);
    let alignment = diagnostics
        .relay_history
        .leading_context_alignment
        .as_ref()
        .unwrap();
    assert_eq!(alignment.shared_prefix_items, 0);
    assert_eq!(alignment.comparison, CacheHistoryComparison::Rewritten);

    let mut client = client;
    client["previous_response_id"] = json!("synthetic_predecessor");
    let observed = begin(&store, &client, "continuation", 20);
    let diagnostics = store.prepare(&observed, &upstream, "candidate-a", 21);
    assert!(
        diagnostics
            .relay_history
            .leading_context_alignment
            .is_none()
    );
    assert_eq!(diagnostics.relay_history.shared_prefix_items, Some(0));
}

#[test]
fn relay_alignment_handles_removed_system_prefix_and_leaves_ordinary_changes_literal() {
    let store = CacheContextStore::default();
    let mut client = payload();
    client["input"][0]["role"] = json!("system");
    let mut upstream = client.clone();
    upstream["input"].as_array_mut().unwrap().remove(0);
    let observed = begin(&store, &client, "removed", 10);
    let diagnostics = store.prepare(&observed, &upstream, "candidate-a", 11);
    let alignment = diagnostics
        .relay_history
        .leading_context_alignment
        .as_ref()
        .unwrap();
    assert_eq!(alignment.skipped_client_items, 1);
    assert_eq!(alignment.skipped_upstream_items, 0);
    assert_eq!(alignment.shared_prefix_items, 1);

    let observed = begin(&store, &client, "unchanged", 20);
    let diagnostics = store.prepare(&observed, &client, "candidate-a", 21);
    assert!(
        diagnostics
            .relay_history
            .leading_context_alignment
            .is_none()
    );
    let mut changed = client.clone();
    changed["input"][1]["content"] = json!("Changed body without a prefix shift.");
    let observed = begin(&store, &client, "body", 30);
    let diagnostics = store.prepare(&observed, &changed, "candidate-a", 31);
    assert!(
        diagnostics
            .relay_history
            .leading_context_alignment
            .is_none()
    );
    assert_eq!(diagnostics.relay_history.shared_prefix_items, Some(1));
}

use super::*;

#[test]
fn retention_archives_metrics_and_rejects_late_duplicate_rows() {
    let root = test_root("usage-retention");
    let path = root.join("relay.sqlite");
    let store = Store::open(path.clone()).unwrap();
    let mut event = UsageEvent {
        request_id: "req_old".into(),
        attempt: 1,
        local_key_id: "key_alpha".into(),
        source_id: "source_alpha".into(),
        candidate_id: Some("source_alpha".into()),
        account_id: None,
        account_token_generation: None,
        client_context_id: None,
        routing: None,
        requested_model: Some("gpt-5.4".into()),
        resolved_model: Some("gpt-5.4".into()),
        requested_reasoning_effort: None,
        effective_reasoning_effort: None,
        wire_api: WireApi::Responses,
        transport: zenith_relay_core::UsageTransport::Http,
        service_tier: DefaultServiceTier::Fast,
        applied_service_tier: Some("priority".into()),
        success: true,
        http_status: 200,
        error_category: None,
        tool_use: ToolUseDiagnostics::default(),
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: Some(0),
        latency_ms: 100,
        ttft_ms: Some(10),
        generation_ms: Some(90),
        input_tokens: Some(1_000_000),
        cached_input_tokens: Some(400_000),
        cache_write_input_tokens: None,
        cache_write_ttl: None,
        reasoning_tokens: Some(0),
        output_tokens: Some(100_000),
        total_tokens: Some(1_100_000),
        upstream_error: None,
        quota_snapshot: None,
    };
    store.record_usage(&event, DAY_MS).unwrap();
    let initial_usage = store.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(initial_usage.totals.generation_output_tokens, 0);
    assert_eq!(initial_usage.totals.generation_samples, 0);
    event.request_id = "req_current".into();
    event.input_tokens = Some(10);
    event.cached_input_tokens = Some(0);
    event.output_tokens = Some(0);
    event.total_tokens = Some(10);
    store.record_usage(&event, 100 * DAY_MS).unwrap();

    assert_eq!(
        store
            .prune_usage_history_with_limits(50 * DAY_MS, 100, 0)
            .unwrap(),
        1
    );
    assert_eq!(store.usage_page(&UsageQuery::default()).unwrap().total, 1);
    let candidate_hint = hex::encode(Sha256::digest(b"source_alpha"))[..12].to_string();
    assert_eq!(
        store.api_equivalents().unwrap()[&candidate_hint].micro_usd,
        3_100_025
    );

    event.request_id = "req_newest".into();
    store.record_usage(&event, 102 * DAY_MS).unwrap();
    assert_eq!(store.prune_usage_history_with_limits(0, 1, 0).unwrap(), 1);
    assert_eq!(
        store.api_equivalents().unwrap()[&candidate_hint].micro_usd,
        3_100_050
    );
    let archived_daily_requests = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT COALESCE(SUM(requests), 0) FROM usage_key_rollups \
             WHERE local_key_id = 'key_alpha' AND period_start_ms >= 0",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(archived_daily_requests, 2);

    event.request_id = "req_old".into();
    event.input_tokens = Some(9_000_000);
    store.record_usage(&event, 101 * DAY_MS).unwrap();
    assert_eq!(store.usage_page(&UsageQuery::default()).unwrap().total, 1);
    assert_eq!(
        store.api_equivalents().unwrap()[&candidate_hint].micro_usd,
        3_100_050
    );
    drop(store);

    let reopened = Store::open(path).unwrap();
    assert_eq!(
        reopened.api_equivalents().unwrap()[&candidate_hint].micro_usd,
        3_100_050
    );
    assert_eq!(reopened.clear_usage().unwrap(), 1);
    assert!(reopened.api_equivalents().unwrap().is_empty());
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_prices_revalue_raw_and_archived_usage_per_provider() {
    let root = test_root("source-prices");
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    let price = |input, output| ApiModelPriceOverride {
        input_micro_usd_per_million: input,
        cached_input_micro_usd_per_million: Some(input / 10),
        cache_write_5m_micro_usd_per_million: None,
        cache_write_1h_micro_usd_per_million: None,
        output_micro_usd_per_million: output,
    };
    for (id, model_price) in [
        ("source_cheap", price(1_000_000, 2_000_000)),
        ("source_expensive", price(2_000_000, 4_000_000)),
    ] {
        store
            .save_source(&SourceRecord {
                id: id.into(),
                name: id.into(),
                enabled: true,
                in_pool: true,
                draining: false,
                base_url: "https://example.test/v1".into(),
                secret_ref: format!("source:{id}"),
                pricing_provider: None,
                official_provider_family: None,
                wire_api: WireApi::Responses,
                protocol_config: Default::default(),
                protocol_bindings: Vec::new(),
                models: vec!["private-model".into()],
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                priority: 0,
                weight: 1,
                recovery_delay_seconds: 0,
                model_price_overrides: BTreeMap::from([("private-model".into(), model_price)]),
                detected_model_prices: BTreeMap::new(),
                last_error_code: None,
            })
            .unwrap();
    }
    let mut event = UsageEvent {
        request_id: "request-cheap".into(),
        attempt: 1,
        local_key_id: "key".into(),
        source_id: "source_cheap".into(),
        candidate_id: Some("source_cheap".into()),
        account_id: None,
        account_token_generation: None,
        client_context_id: None,
        routing: None,
        requested_model: Some("private-model".into()),
        resolved_model: Some("private-model".into()),
        requested_reasoning_effort: None,
        effective_reasoning_effort: None,
        wire_api: WireApi::Responses,
        transport: zenith_relay_core::UsageTransport::Http,
        service_tier: DefaultServiceTier::Standard,
        applied_service_tier: None,
        success: true,
        http_status: 200,
        error_category: None,
        tool_use: ToolUseDiagnostics::default(),
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: Some(0),
        latency_ms: 100,
        ttft_ms: Some(10),
        generation_ms: Some(90),
        input_tokens: Some(1_000_000),
        cached_input_tokens: Some(0),
        cache_write_input_tokens: Some(0),
        cache_write_ttl: None,
        reasoning_tokens: Some(0),
        output_tokens: Some(100_000),
        total_tokens: Some(1_100_000),
        upstream_error: None,
        quota_snapshot: None,
    };
    store.record_usage(&event, 1).unwrap();
    event.request_id = "request-expensive".into();
    event.source_id = "source_expensive".into();
    event.candidate_id = Some("source_expensive".into());
    store.record_usage(&event, 1).unwrap();

    let page = store.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(page.totals.api_equivalent.micro_usd, 3_600_000);
    let event_value = |request_id: &str| {
        page.events
            .iter()
            .find(|event| event.request_id == request_id)
            .unwrap()
            .api_equivalent
    };
    assert_eq!(event_value("request-cheap").micro_usd, 1_200_000);
    assert_eq!(event_value("request-expensive").micro_usd, 2_400_000);
    let equivalents = store.api_equivalents().unwrap();
    assert_eq!(
        equivalents[&identity_hint("source_cheap")].micro_usd,
        1_200_000
    );
    assert_eq!(
        equivalents[&identity_hint("source_expensive")].micro_usd,
        2_400_000
    );

    assert_eq!(store.prune_usage_history_with_limits(2, 100, 0).unwrap(), 2);
    assert_eq!(
        store
            .api_equivalents()
            .unwrap()
            .values()
            .map(|value| value.micro_usd)
            .sum::<u64>(),
        3_600_000
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn usage_filters_paginate_escape_wildcards_and_clear() {
    use zenith_relay_core::protocol::UsageRange;

    let root = test_root("usage-query");
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    for (index, success, model, error) in [
        (1, true, "gpt-5.4", None),
        (2, false, "gpt%literal", Some("quota_exhausted")),
        (3, true, "gpt-test", None),
    ] {
        store
            .record_usage(
                &UsageEvent {
                    request_id: format!("req_{index}"),
                    attempt: 1,
                    local_key_id: "key_alpha".to_string(),
                    source_id: "source_alpha".to_string(),
                    candidate_id: Some("source_alpha".to_string()),
                    account_id: None,
                    account_token_generation: None,
                    client_context_id: None,
                    routing: Some(RoutingDiagnostics {
                        reason: SelectionReason::QuotaHeadroom,
                        eligible_candidates: 3,
                        quota_remaining_basis_points: Some(5_400),
                        in_flight_before: 0,
                        dispatches_before: index - 1,
                        endpoint_kind: None,
                    }),
                    requested_model: Some(model.to_string()),
                    resolved_model: Some(model.to_string()),
                    requested_reasoning_effort: None,
                    effective_reasoning_effort: None,
                    wire_api: WireApi::Responses,
                    transport: zenith_relay_core::UsageTransport::Http,
                    service_tier: DefaultServiceTier::Standard,
                    applied_service_tier: None,
                    success,
                    http_status: if success { 200 } else { 429 },
                    error_category: error.map(str::to_string),
                    tool_use: ToolUseDiagnostics::default(),
                    cooldown_scope: None,
                    retry_at_ms: None,
                    consecutive_failures: None,
                    latency_ms: 10,
                    ttft_ms: Some(4),
                    generation_ms: Some(6),
                    input_tokens: Some(1),
                    cached_input_tokens: Some(u64::from(index != 2)),
                    cache_write_input_tokens: (index == 2).then_some(1),
                    cache_write_ttl: None,
                    reasoning_tokens: Some(1),
                    output_tokens: Some(1),
                    total_tokens: Some(2),
                    upstream_error: None,
                    quota_snapshot: None,
                },
                2_000 + index,
            )
            .unwrap();
    }

    let page = store
        .usage_page(&UsageQuery {
            page: 1,
            page_size: 1,
            range: Some(UsageRange::Custom),
            from_ms: Some(2_000),
            to_ms: Some(3_000),
            bucket_ms: Some(1_000),
            model_query: Some("%".to_string()),
            success: Some(false),
            error_category: Some("quota_exhausted".to_string()),
            request_id_query: Some("req_2".to_string()),
            ..UsageQuery::default()
        })
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.total_pages, 1);
    assert_eq!(page.totals.requests, 1);
    assert_eq!(page.totals.total_tokens, 2);
    assert_eq!(page.totals.speed_output_tokens, 0);
    assert_eq!(page.models.len(), 1);
    assert_eq!(page.pool_members.len(), 1);
    assert_eq!(page.buckets.len(), 1);
    assert_eq!(page.buckets[0].start_ms, 2_000);
    assert_eq!(page.buckets[0].totals.total_tokens, 2);
    assert_eq!(
        page.buckets[0].totals.api_equivalent,
        page.totals.api_equivalent
    );
    assert_eq!(page.events[0].request_id, "req_2");
    assert_eq!(page.events[0].ttft_ms, Some(4));
    assert_eq!(page.events[0].tokens.cache_write_input_tokens, Some(1));
    assert_eq!(page.events[0].api_equivalent, page.totals.api_equivalent);
    assert_eq!(page.totals.cache_write_input_tokens, 1);
    assert_eq!(page.totals.cache_write_input_samples, 1);
    assert_eq!(
        page.events[0]
            .routing
            .as_ref()
            .map(|routing| routing.reason),
        Some(SelectionReason::QuotaHeadroom)
    );
    let hint = hex::encode(Sha256::digest(b"source_alpha"))[..12].to_string();
    assert_eq!(
        store.api_equivalents().unwrap().get(&hint),
        Some(&ApiEquivalentSummary {
            micro_usd: 17,
            priced_tokens: 4,
            unpriced_tokens: 2,
        })
    );
    assert_eq!(store.clear_usage().unwrap(), 3);
    assert_eq!(store.usage_page(&UsageQuery::default()).unwrap().total, 0);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

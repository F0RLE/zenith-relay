use super::*;
use zenith_relay_core::UsageTransport;

#[test]
fn usage_filter_options_cover_history_independently_of_pages_and_filters() {
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    for (id, wire_api, transport, category) in [
        (
            "first",
            WireApi::Responses,
            UsageTransport::Http,
            "upstream_failure",
        ),
        (
            "second",
            WireApi::Messages,
            UsageTransport::Websocket,
            "rate_limited",
        ),
        (
            "third",
            WireApi::Gemini,
            UsageTransport::Http,
            "upstream_transport",
        ),
    ] {
        let mut event = aggregate_test_event(id, 1, 20, 0, None, 8);
        event.wire_api = wire_api;
        event.transport = transport;
        event.success = false;
        event.error_category = Some(category.into());
        event.requested_model = Some(format!("requested-{id}"));
        event.resolved_model = Some(format!("served-{id}"));
        event.source_id = format!("source-{id}");
        event.account_id = (wire_api == WireApi::Messages).then(|| format!("account-{id}"));
        database.record(&event).unwrap();
    }
    let query = UsageQuery {
        page_size: 1,
        include_filter_options: true,
        ..UsageQuery::default()
    };
    let first = database.usage_page(&query).unwrap();
    assert_eq!(first.events.len(), 1);
    assert_eq!(first.total_pages, 3);
    let options = first.filter_options.unwrap();
    assert_eq!(
        options.models,
        [
            "requested-first",
            "requested-second",
            "requested-third",
            "served-first",
            "served-second",
            "served-third",
        ]
    );
    assert_eq!(
        options.pool_members,
        ["account-second", "source-first", "source-third"]
    );
    assert_eq!(options.wire_apis, ["gemini", "messages", "responses"]);
    assert_eq!(options.transports, ["http", "websocket"]);
    assert_eq!(
        options.error_categories,
        ["rate_limited", "upstream_failure", "upstream_transport"]
    );
    let empty = database
        .usage_page(&UsageQuery {
            page: 2,
            wire_api: Some(WireApi::Messages),
            transport: Some(UsageTransport::Http),
            error_category: Some("upstream_failure".into()),
            source_or_account_query: Some("missing-source".into()),
            model_query: Some("missing-model".into()),
            request_id_query: Some("missing-request".into()),
            success: Some(true),
            ..query
        })
        .unwrap();
    assert!(empty.events.is_empty());
    assert_eq!(empty.total, 0);
    let serialized = serde_json::to_value(empty).unwrap();
    assert_eq!(
        serialized["filterOptions"]["models"],
        serde_json::json!(options.models)
    );
    assert_eq!(
        serialized["filterOptions"]["poolMembers"],
        serde_json::json!(options.pool_members)
    );
    assert_eq!(
        serialized["filterOptions"]["wireApis"],
        serde_json::json!(options.wire_apis)
    );
    assert_eq!(
        serialized["filterOptions"]["transports"],
        serde_json::json!(options.transports)
    );
    assert_eq!(
        serialized["filterOptions"]["errorCategories"],
        serde_json::json!(options.error_categories)
    );
    assert!(database
        .usage_page(&UsageQuery::default())
        .unwrap()
        .filter_options
        .is_none());
}

#[test]
fn usage_filter_options_follow_period_bounds_and_clear_with_history() {
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    for (id, date, wire_api, transport, category) in [
        (
            "before",
            "2026-10-21 12:00:00",
            WireApi::Responses,
            UsageTransport::Http,
            "upstream_failure",
        ),
        (
            "inside",
            "2026-10-23 12:00:00",
            WireApi::Messages,
            UsageTransport::Websocket,
            "rate_limited",
        ),
        (
            "after",
            "2026-10-25 12:00:00",
            WireApi::Gemini,
            UsageTransport::Http,
            "upstream_transport",
        ),
    ] {
        let mut event = aggregate_test_event(id, 1, 20, 0, None, 8);
        event.wire_api = wire_api;
        event.transport = transport;
        event.error_category = Some(category.into());
        event.requested_model = Some(format!("requested-{id}"));
        event.resolved_model = Some(format!("served-{id}"));
        event.source_id = format!("source-{id}");
        database.record(&event).unwrap();
        database
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE request_logs SET created_at = ?1 WHERE request_id = ?2",
                params![date, id],
            )
            .unwrap();
        database
            .connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE request_attempts SET created_at = ?1 WHERE request_id = ?2",
                params![date, id],
            )
            .unwrap();
    }
    let timestamp = |date: &str| {
        chrono::DateTime::parse_from_rfc3339(date)
            .unwrap()
            .timestamp_millis() as u64
    };
    let query = UsageQuery {
        from_ms: Some(timestamp("2026-10-22T00:00:00Z")),
        to_ms: Some(timestamp("2026-10-24T00:00:00Z")),
        include_filter_options: true,
        ..UsageQuery::default()
    };
    let options = database.usage_page(&query).unwrap().filter_options.unwrap();
    assert_eq!(options.models, ["requested-inside", "served-inside"]);
    assert_eq!(options.pool_members, ["source-inside"]);
    assert_eq!(options.wire_apis, ["messages"]);
    assert_eq!(options.transports, ["websocket"]);
    assert_eq!(options.error_categories, ["rate_limited"]);
    database.clear().unwrap();
    let options = database.usage_page(&query).unwrap().filter_options.unwrap();
    assert!(options.models.is_empty());
    assert!(options.pool_members.is_empty());
    assert!(options.wire_apis.is_empty());
    assert!(options.transports.is_empty());
    assert!(options.error_categories.is_empty());
}

#[test]
fn usage_filter_options_normalize_legacy_protocols_and_skip_empty_categories() {
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    let mut event = aggregate_test_event("current", 1, 20, 0, None, 8);
    event.wire_api = WireApi::ChatCompletions;
    database.record(&event).unwrap();
    event.request_id = "legacy".into();
    event.error_category = Some(String::new());
    database.record(&event).unwrap();
    database
        .connection
        .lock()
        .unwrap()
        .execute(
            "UPDATE request_logs SET wire_api = 'chatcompletions' WHERE request_id = 'legacy'",
            [],
        )
        .unwrap();
    let options = database
        .usage_page(&UsageQuery {
            include_filter_options: true,
            ..UsageQuery::default()
        })
        .unwrap()
        .filter_options
        .unwrap();
    assert_eq!(options.wire_apis, ["chat_completions"]);
    assert_eq!(options.transports, ["http"]);
    assert!(options.error_categories.is_empty());
}

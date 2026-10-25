use super::*;
use std::sync::atomic::Ordering;

fn read(database: &TelemetryDb, query: &UsageQuery) -> UsageFilterOptions {
    let connection = database.connection.lock().unwrap();
    database
        .cached_usage_filter_options(&connection, query)
        .unwrap()
}

fn reads(database: &TelemetryDb) -> u64 {
    database.usage_filter_option_reads.load(Ordering::Relaxed)
}

#[test]
fn filter_choices_reuse_history_across_pages_searches_and_report_tabs() {
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    let mut event = aggregate_test_event("filter-cache", 1, 20, 0, None, 8);
    event.requested_model = Some("synthetic-old-model".into());
    event.resolved_model = event.requested_model.clone();
    database.record(&event).unwrap();
    let query = UsageQuery::default();
    let original = read(&database, &query);
    assert_eq!(reads(&database), 1);
    let changed = read(
        &database,
        &UsageQuery {
            page: 2,
            model_query: Some("missing".into()),
            request_id_query: Some("missing".into()),
            success: Some(false),
            include_events: Some(false),
            include_models: Some(true),
            ..query.clone()
        },
    );
    assert_eq!(
        serde_json::to_value(changed).unwrap(),
        serde_json::to_value(original).unwrap()
    );
    assert_eq!(reads(&database), 1);

    event.requested_model = Some("synthetic-corrected-model".into());
    event.resolved_model = event.requested_model.clone();
    database.record(&event).unwrap();
    assert_eq!(
        read(&database, &query).models,
        ["synthetic-corrected-model"]
    );
    assert_eq!(reads(&database), 2);
    database.clear().unwrap();
    assert!(read(&database, &query).models.is_empty());
    assert_eq!(reads(&database), 3);
}

#[test]
fn filter_cache_keeps_only_the_current_period_and_refreshes_after_retention() {
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    database
        .record(&aggregate_test_event("retained-filter", 1, 20, 0, None, 8))
        .unwrap();
    let query = UsageQuery::default();
    assert!(!read(&database, &query).models.is_empty());
    let bounded = UsageQuery {
        from_ms: Some(u64::MAX / 2),
        ..query.clone()
    };
    assert!(read(&database, &bounded).models.is_empty());
    assert_eq!(reads(&database), 2);
    assert!(!read(&database, &query).models.is_empty());
    assert_eq!(reads(&database), 3);
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch(
            "UPDATE request_logs SET created_at = '2000-01-01 00:00:00';
         UPDATE request_attempts SET created_at = '2000-01-01 00:00:00';",
        )
        .unwrap();
    assert_eq!(database.maintain_usage().unwrap(), 1);
    assert!(read(&database, &query).models.is_empty());
    assert_eq!(reads(&database), 4);
}

#[test]
fn account_purge_invalidates_cached_filter_choices() {
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    let mut event = aggregate_test_event("purged-filter", 1, 20, 0, None, 8);
    event.account_id = Some("synthetic-filter-account".into());
    database.record(&event).unwrap();
    assert_eq!(
        read(&database, &UsageQuery::default()).pool_members,
        ["synthetic-filter-account"]
    );
    database
        .replace_state_json_and_delete_accounts_data(&[], &["synthetic-filter-account".into()])
        .unwrap();
    assert!(read(&database, &UsageQuery::default())
        .pool_members
        .is_empty());
    assert_eq!(reads(&database), 2);
}

#[test]
fn filter_choices_use_covering_indexes_for_all_time_and_period_reads() {
    let database = TelemetryDb::open(Path::new(":memory:")).unwrap();
    let connection = database.connection.lock().unwrap();
    for query in [
        UsageQuery::default(),
        UsageQuery {
            from_ms: Some(1),
            to_ms: Some(2),
            ..UsageQuery::default()
        },
    ] {
        let (where_sql, parameters) = usage_filter(&query);
        let history = super::super::usage::send_projection(
            "wire_api, transport, error_category, requested_model, resolved_model, account_id, source_id",
            &where_sql,
        );
        let mut statement = connection
            .prepare(&format!(
                "EXPLAIN QUERY PLAN SELECT DISTINCT CASE WHEN wire_api = 'chatcompletions'
             THEN 'chat_completions' ELSE wire_api END, transport, error_category,
             requested_model, resolved_model, COALESCE(account_id, source_id, '') FROM ({history})"
            ))
            .unwrap();
        let plan = statement
            .query_map(
                params_from_iter(parameters.iter().chain(parameters.iter())),
                |row| row.get::<_, String>(3),
            )
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap()
            .join("\n");
        assert!(
            plan.contains("COVERING INDEX request_attempts_filter_options"),
            "{plan}"
        );
        assert!(
            plan.contains("COVERING INDEX request_logs_filter_options"),
            "{plan}"
        );
    }
}

#[test]
fn filter_read_migration_upgrades_existing_history_without_rewriting_it() {
    let root =
        std::env::temp_dir().join(format!("relay-filter-migration-{}", uuid::Uuid::new_v4()));
    let path = root.join("usage.sqlite");
    let database = TelemetryDb::open(&path).unwrap();
    database
        .record(&aggregate_test_event("migration-filter", 1, 20, 0, None, 8))
        .unwrap();
    let expected = read(&database, &UsageQuery::default());
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch(
            "DROP INDEX request_attempts_filter_options;
         DROP INDEX request_logs_filter_options;
         PRAGMA user_version = 38;",
        )
        .unwrap();
    drop(database);
    let database = TelemetryDb::open(&path).unwrap();
    let actual = read(&database, &UsageQuery::default());
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(database.list(10).unwrap().len(), 1);
    let version: u32 = database
        .connection
        .lock()
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, LOCAL_DATABASE_SCHEMA_VERSION);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

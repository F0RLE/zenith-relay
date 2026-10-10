use super::*;

#[test]
fn telemetry_uses_wal_without_relaxing_durability() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-telemetry-pragmas-{}",
        uuid::Uuid::new_v4()
    ));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let connection = database.connection.lock().unwrap();
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    let synchronous: u8 = connection
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .unwrap();
    let auto_checkpoint: u32 = connection
        .query_row("PRAGMA wal_autocheckpoint", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal_mode, "wal");
    assert_eq!(synchronous, 2);
    assert_eq!(auto_checkpoint, 1_000);
    drop(connection);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn deleting_account_data_removes_usage_rollups_and_affinity() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-account-delete-{}",
        uuid::Uuid::new_v4()
    ));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO request_logs(
                    request_id, local_key_id, source_id, candidate_id, account_id,
                    wire_api, success, http_status, latency_ms
                 ) VALUES ('request-delete', 'key', 'codex', 'account-delete',
                    'account-delete', 'responses', 1, 200, 1);
                 INSERT INTO usage_candidate_rollups(candidate_kind, candidate_id, model)
                 VALUES ('account', 'account-delete', 'gpt-test');
                 INSERT INTO response_affinity(
                    response_key, candidate_id, expires_at_ms, updated_at_ms
                 ) VALUES ('response-delete', 'account-delete', 1000, 1);",
        )
        .unwrap();

    database
        .replace_state_json_and_delete_account_data(
            &[("accounts", "[]".to_string())],
            "account-delete",
        )
        .unwrap();

    let remaining: i64 = database
        .connection
        .lock()
        .unwrap()
        .query_row(
            "SELECT
                    (SELECT COUNT(*) FROM request_logs WHERE account_id = 'account-delete') +
                    (SELECT COUNT(*) FROM usage_candidate_rollups
                     WHERE candidate_kind = 'account' AND candidate_id = 'account-delete') +
                    (SELECT COUNT(*) FROM response_affinity
                     WHERE candidate_id = 'account-delete')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 0);
    assert_eq!(
        database.state_json("accounts").unwrap().as_deref(),
        Some("[]")
    );
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn deleting_multiple_accounts_removes_all_usage_data_in_one_transaction() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-account-bulk-delete-{}",
        uuid::Uuid::new_v4()
    ));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    database
        .connection
        .lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO request_logs(
                    request_id, local_key_id, source_id, candidate_id, account_id,
                    wire_api, success, http_status, latency_ms
                 ) VALUES
                    ('request-delete-1', 'key', 'codex', 'account-delete-1',
                     'account-delete-1', 'responses', 1, 200, 1),
                    ('request-delete-2', 'key', 'codex', 'account-delete-2',
                     'account-delete-2', 'responses', 1, 200, 1);
                 INSERT INTO usage_candidate_rollups(candidate_kind, candidate_id, model)
                 VALUES ('account', 'account-delete-1', 'gpt-test'),
                        ('account', 'account-delete-2', 'gpt-test');
                 INSERT INTO response_affinity(
                     response_key, candidate_id, expires_at_ms, updated_at_ms
                 ) VALUES
                    ('response-delete-1', 'account-delete-1', 1000, 1),
                    ('response-delete-2', 'account-delete-2', 1000, 1);",
        )
        .unwrap();

    database
        .replace_state_json_and_delete_accounts_data(
            &[("accounts", "[]".to_string())],
            &[
                "account-delete-1".to_string(),
                "account-delete-2".to_string(),
            ],
        )
        .unwrap();

    let remaining: i64 = database
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT
                    (SELECT COUNT(*) FROM request_logs WHERE account_id LIKE 'account-delete-%') +
                    (SELECT COUNT(*) FROM usage_candidate_rollups
                     WHERE candidate_kind = 'account' AND candidate_id LIKE 'account-delete-%') +
                    (SELECT COUNT(*) FROM response_affinity WHERE candidate_id LIKE 'account-delete-%')",
                [],
                |row| row.get(0),
            )
            .unwrap();
    assert_eq!(remaining, 0);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn invalid_state_batch_does_not_partially_save_or_purge_account_data() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-account-delete-rollback-{}",
        uuid::Uuid::new_v4()
    ));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    database
        .replace_state_json(&[("accounts", "[\"before\"]".to_string())])
        .unwrap();
    database
        .connection
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO request_logs(
                    request_id, local_key_id, source_id, candidate_id, account_id,
                    wire_api, success, http_status, latency_ms
                 ) VALUES ('request-rollback', 'key', 'codex', 'account-rollback',
                    'account-rollback', 'responses', 1, 200, 1)",
            [],
        )
        .unwrap();

    let error = database
        .replace_state_json_and_delete_account_data(
            &[
                ("accounts", "[]".to_string()),
                ("invalid-key", "{}".to_string()),
            ],
            "account-rollback",
        )
        .unwrap_err();

    assert_eq!(error.code, ErrorCode::InvalidState);
    assert_eq!(
        database.state_json("accounts").unwrap().as_deref(),
        Some("[\"before\"]")
    );
    let remaining: i64 = database
        .connection
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM request_logs WHERE account_id = 'account-rollback'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(remaining, 1);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn usage_schema_has_no_secret_or_body_columns() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-usage-schema-{}",
        uuid::Uuid::new_v4()
    ));
    let database = TelemetryDb::open(&root.join("usage.sqlite")).unwrap();
    let connection = database.connection.lock().unwrap();
    let mut statement = connection
        .prepare("SELECT name FROM pragma_table_info('request_logs')")
        .unwrap();
    let columns = statement
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert!(!columns.iter().any(|column| {
        let column = column.to_lowercase();
        column.contains("secret")
            || column.contains("prompt")
            || column.contains("request_body")
            || column.contains("response_body")
    }));
    drop(statement);
    drop(connection);
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn newer_usage_schema_is_rejected_without_rewrite() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-usage-future-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("usage.sqlite");
    let connection = Connection::open(&path).unwrap();
    let future_version = LOCAL_DATABASE_SCHEMA_VERSION + 1;
    connection
        .pragma_update(None, "user_version", future_version)
        .unwrap();
    drop(connection);

    assert!(matches!(
        TelemetryDb::open(&path).err().unwrap().code,
        ErrorCode::UnsupportedSchema
    ));
    let version: u32 = Connection::open(&path)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, future_version);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn usage_retention_prunes_old_rows_on_open_and_periodically() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-usage-retention-{}",
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("usage.sqlite");
    drop(TelemetryDb::open(&path).unwrap());
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO request_logs (
                    request_id, attempt, local_key_id, source_id, candidate_id, account_id,
                    requested_model, resolved_model, wire_api, success, http_status, latency_ms,
                    input_tokens, cached_input_tokens, output_tokens, total_tokens, created_at
                ) VALUES ('old-open', 1, 'key', 'source', 'account', 'account',
                    'gpt-5.4', 'gpt-5.4', 'responses', 1, 200, 1, 20, 10, 8, 28,
                    datetime('now', '-31 days'))",
            [],
        )
        .unwrap();
    drop(connection);

    let database = TelemetryDb::open(&path).unwrap();
    assert!(database.list(10).unwrap().is_empty());
    assert_eq!(
        database.api_equivalents().unwrap().accounts.get("account"),
        Some(&ApiEquivalentSummary {
            micro_usd: 148,
            priced_tokens: 28,
            unpriced_tokens: 0,
        })
    );
    database
        .connection
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO request_logs (
                    id, request_id, attempt, local_key_id, source_id, candidate_id,
                    requested_model, resolved_model, wire_api, success, http_status, latency_ms,
                    input_tokens, cached_input_tokens, output_tokens, total_tokens, created_at
                ) VALUES (255, 'old-trigger', 1, 'key', 'source', 'source',
                    'gpt-5.4', 'gpt-5.4', 'responses', 1, 200, 1, 20, 10, 8, 28,
                    datetime('now', '-31 days'))",
            [],
        )
        .unwrap();
    database
        .record(&UsageEvent {
            request_id: "trigger-256".into(),
            attempt: 1,
            local_key_id: "key".into(),
            source_id: "source".into(),
            candidate_id: None,
            account_id: None,
            account_token_generation: None,
            client_context_id: None,
            routing: None,
            requested_model: None,
            resolved_model: None,
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
        })
        .unwrap();
    assert_eq!(database.list(10).unwrap().len(), 1);

    // A later upsert must not inherit the trigger rowid and run the
    // retention scan again. Leave a fresh old row behind to detect that
    // accidental rescan.
    database
        .connection
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO request_logs (
                    request_id, attempt, local_key_id, source_id, candidate_id,
                    requested_model, resolved_model, wire_api, success, http_status,
                    latency_ms, input_tokens, output_tokens, total_tokens, created_at
                 ) VALUES ('old-after-trigger', 1, 'key', 'source', 'source',
                    'gpt-5.4', 'gpt-5.4', 'responses', 1, 200, 1, 20, 8, 28,
                    datetime('now', '-31 days'))",
            [],
        )
        .unwrap();
    database
        .connection
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO performance_samples(id, name, duration_ms)
                 VALUES (256, 'retention-regression', 1.0)",
            [],
        )
        .unwrap();
    database
        .record(&aggregate_test_event("trigger-256", 2, 0, 0, None, 0))
        .unwrap();
    assert_eq!(
        database
            .connection
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM request_logs WHERE request_id = 'old-after-trigger'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        database.api_equivalents().unwrap().sources.get("source"),
        Some(&ApiEquivalentSummary {
            micro_usd: 148,
            priced_tokens: 28,
            unpriced_tokens: 0,
        })
    );
    database.clear().unwrap();
    let equivalents = database.api_equivalents().unwrap();
    assert!(equivalents.accounts.is_empty());
    assert!(equivalents.sources.is_empty());
    drop(database);
    std::fs::remove_dir_all(root).unwrap();
}

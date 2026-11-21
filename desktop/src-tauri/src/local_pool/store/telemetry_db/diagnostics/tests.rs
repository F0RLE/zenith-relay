use super::*;
use serde_json::json;

#[test]
fn verbose_expiry_keeps_control_flags_usage_and_required_tool_fields() {
    let routing = json!({
        "endpointKind":"responses", "attemptOutcome":"client_disconnected",
        "cacheContext":{"inputTokens":1000}, "cacheAffinity":{"outcome":"reused"},
        "routeLatency":{"totalMs":10}, "transport":{"upstream":"http"},
        "requestTiming":{"durationMs":100,"ttftMs":2,"projectionOnly":true,
            "waiting":{"totalMs":80}},
        "providerBackoff":{"seconds":30}
    })
    .to_string();
    assert_eq!(
        retained_json(Some(&routing), false, true).as_deref(),
        Some(routing.as_str())
    );
    let retained: serde_json::Value =
        serde_json::from_str(&retained_json(Some(&routing), true, true).unwrap()).unwrap();
    assert_eq!(
        retained,
        json!({
            "endpointKind":"responses", "attemptOutcome":"client_disconnected",
            "requestTiming":{"durationMs":100,"ttftMs":2,"projectionOnly":true},
            "providerBackoff":{"seconds":30}
        })
    );
    let tools = json!({
        "clientToolCount":2,"forwardedToolCount":2,"toolCallCount":1,
        "terminalOutput":"tool_calls", "providerUsage":{"cacheReadInputTokens":100},
        "policyOutcome":"forwarded", "clientContext":{"bodyBytes":100},
        "upstreamContext":{"bodyBytes":90}, "basisPointsCatalog":{"preloaded":2},
        "clientSchemaBytes":80,"forwardedSchemaBytes":80
    })
    .to_string();
    let retained: serde_json::Value =
        serde_json::from_str(&retained_json(Some(&tools), true, false).unwrap()).unwrap();
    assert_eq!(
        retained,
        json!({
            "clientToolCount":2,"forwardedToolCount":2,"toolCallCount":1,
            "terminalOutput":"tool_calls", "providerUsage":{"cacheReadInputTokens":100},
            "policyOutcome":"forwarded"
        })
    );
    assert_eq!(retained_json(Some("{"), true, true).as_deref(), Some("{"));
    assert_eq!(retained_json(None, true, false), None);
}

#[test]
fn dictionary_rejects_mismatched_fingerprints_instead_of_reusing_different_evidence() {
    let mut connection = rusqlite::Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE usage_diagnostics(id INTEGER PRIMARY KEY,
            fingerprint BLOB UNIQUE, data_json TEXT, refs INTEGER DEFAULT 0)",
        )
        .unwrap();
    let transaction = connection.transaction().unwrap();
    let id = store_json(&transaction, Some("{}")).unwrap().unwrap();
    transaction
        .execute(
            "UPDATE usage_diagnostics SET data_json = '[]' WHERE id = ?1",
            [id],
        )
        .unwrap();
    assert!(store_json(&transaction, Some("{}")).is_err());
    assert_eq!(store_json(&transaction, None).unwrap(), None);
    transaction.rollback().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM usage_diagnostics", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

use std::env;
use std::fs;

use super::persist::{
    clear_last_stages, ensure_layout, persist_last_stage_at, read_debug_marker, read_latest_stage,
};
use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn identifiers_are_stable_but_not_reversible() {
    let first = hash_identifier("account-synthetic");
    assert_eq!(first, hash_identifier("account-synthetic"));
    assert!(first.starts_with("id_"));
    assert!(!first.contains("account"));
}

#[test]
fn redaction_removes_common_credentials_and_multiline_text() {
    let text = safe_text(
        "Bearer synthetic-token\nemail@example.test sk-1234567890abcdef",
        MAX_TEXT_BYTES,
    );
    assert!(!text.contains("synthetic-token"));
    assert!(!text.contains("email@example.test"));
    assert!(!text.contains("sk-1234567890abcdef"));
    assert!(!text.contains('\n'));
}

#[test]
fn redaction_handles_escaped_json_values_and_identity_fields() {
    let text = safe_text(
        r#"{"api_key":"synthetic-\"quoted\"-secret","account_id":"account_private_123456","user_id":"user-opaque-value-123456789","authorization":"Bearer synthetic-bearer","url":"https://login-name@example.test/v1?state=ok&code=oauth-secret#done"}"#,
        MAX_TEXT_BYTES,
    );
    assert!(!text.contains("synthetic-"));
    assert!(!text.contains("account_private_123456"));
    assert!(!text.contains("user-opaque-value-123456789"));
    assert!(!text.contains("login-name@example.test"));
    assert!(!text.contains("Bearer synthetic-bearer"));
    assert!(text.contains("https://[redacted]@example.test/v1"));
    assert!(text.contains("?state=ok&code=[redacted]#done"));
}

#[test]
fn redaction_covers_camel_case_fields_and_every_query_segment() {
    let text = safe_text(
        r#"{"accessToken":"camel-secret","refreshToken":"refresh-secret","apiKey":"key-secret","safe":"visible","url":"https://example.test/path?flag&token=query-secret&safe=visible&code=second-secret"}"#,
        MAX_TEXT_BYTES,
    );
    assert!(!text.contains("camel-secret"));
    assert!(!text.contains("refresh-secret"));
    assert!(!text.contains("key-secret"));
    assert!(!text.contains("query-secret"));
    assert!(!text.contains("second-secret"));
    assert!(text.contains("safe"));
    assert!(text.contains("?flag&token=[redacted]&safe=visible&code=[redacted]"));
}

#[test]
fn redaction_keeps_operation_names_and_hides_identity_like_tokens() {
    assert_eq!(
        safe_text("account-import", MAX_TEXT_BYTES),
        "account-import"
    );
    let account = safe_text("account_private_123456", MAX_TEXT_BYTES);
    assert!(account.starts_with("account_"));
    assert!(!account.contains("private_123456"));
    assert_eq!(safe_text("acct-42", MAX_TEXT_BYTES), "acct-[redacted]");
}

#[test]
fn diagnostic_codes_remain_actionable_without_allowing_free_form_text() {
    assert_eq!(
        safe_code(" source_protocol_invalid "),
        "source_protocol_invalid"
    );
    assert_eq!(
        safe_code("Gateway-Restart.Failed"),
        "gateway-restart.failed"
    );
    assert_eq!(safe_code("source account invalid"), REDACTED);
    assert_eq!(safe_code("https://example.test/?token=secret"), REDACTED);
}

#[test]
fn redaction_hides_embedded_uuid_identifiers_without_losing_operation_context() {
    let value = safe_text(
        "delete-account-123e4567-e89b-12d3-a456-426614174000 failed",
        MAX_TEXT_BYTES,
    );
    assert_eq!(value, "delete-account-[redacted] failed");
}

#[test]
fn truncation_respects_utf8_byte_limits() {
    assert_eq!(truncate_text("abcdef", 0), "");
    let value = truncate_text("Привет мир", 8);
    assert!(value.len() <= 8);
    assert!(value.ends_with('…'));
}

#[test]
fn layout_uses_separate_error_crash_and_operation_directories() {
    let root = env::temp_dir().join(format!(
        "zenith-relay-diagnostics-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    ensure_layout(&root);
    assert!(root.join("logs/errors").is_dir());
    assert!(root.join("logs/crashes").is_dir());
    assert!(root.join("logs/operations").is_dir());
    assert!(root.join("logs/README.txt").is_file());
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn debug_marker_is_disabled_by_default_and_enabled_by_presence() {
    let root = env::temp_dir().join(format!(
        "zenith-relay-debug-marker-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    assert!(ensure_layout(&root));
    assert!(!read_debug_marker(&root).expect("default marker state"));
    fs::write(root.join(LOGS_DIRECTORY).join(DEBUG_MARKER_FILE), b"").expect("marker");
    assert!(read_debug_marker(&root).expect("enabled marker state"));
    fs::remove_dir_all(root).expect("cleanup");
}

#[test]
fn interrupted_stage_marker_survives_and_is_cleared_after_clean_shutdown() {
    let root = env::temp_dir().join(format!(
        "zenith-relay-stage-marker-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    assert!(ensure_layout(&root));
    let value = Breadcrumb {
        timestamp: "2026-09-15T12:34:56.000Z".to_string(),
        operation: "account-import".to_string(),
        stage: "runtime_sync_started".to_string(),
        details: BTreeMap::from([("account".to_string(), "id_synthetic".to_string())]),
    };
    persist_last_stage_at(&root, &value);
    let loaded = read_latest_stage(&root).expect("stage marker");
    assert_eq!(loaded.operation, value.operation);
    assert_eq!(loaded.stage, value.stage);
    assert_eq!(loaded.details, value.details);
    clear_last_stages(&root);
    assert!(read_latest_stage(&root).is_none());
    fs::remove_dir_all(root).expect("cleanup");
}

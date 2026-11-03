use super::*;
use zenith_relay_core::accounts::{AccountAuthMode, AccountAuthState, ReauthReason};

fn account_record() -> LocalAccountRecord {
    serde_json::from_value(json!({
        "account": {
            "id":"account_synthetic", "label":"Synthetic",
            "identity":{"stableIndex":"synthetic", "identityHash":"synthetic"},
            "authMode":AccountAuthMode::OAuth, "authState":{"state":"active"},
            "health":"healthy", "sourceId":"synthetic", "secretRefs":[],
            "subscription":zenith_relay_core::quota::Subscription::default(),
            "quota":zenith_relay_core::quota::QuotaSnapshot::default(),
            "tokenGeneration":1, "tags":[], "enabled":true, "inPool":false,
            "draining":false, "createdAtMs":1
        },
        "wireApi":"responses", "models":[]
    }))
    .unwrap()
}

#[test]
fn authorization_diagnostic_preserves_metadata_monitoring_eligibility() {
    let record = account_record();
    assert!(!record.account.in_pool);
    assert!(eligible(&record));
    let mut changed = record.clone();
    changed.account.auth_state = AccountAuthState::DegradedAccessOnly;
    assert!(eligible(&changed));
    changed.account.auth_state = AccountAuthState::RequiresReauth(ReauthReason::AccessTokenExpired);
    assert!(!eligible(&changed));
    let mut changed = record.clone();
    changed.account.enabled = false;
    assert!(!eligible(&changed));
    let mut changed = record.clone();
    changed.account.draining = true;
    assert!(!eligible(&changed));
    let mut changed = record;
    changed.recovery = Some(json!({"kind":"synthetic"}));
    assert!(!eligible(&changed));
}

#[test]
fn authorization_diagnostic_fences_registration_but_allows_usage_updates() {
    let record = account_record();
    let mut changed = record.clone();
    changed.account.last_used_at_ms = Some(123);
    assert!(same_registration(&changed, &record));
    changed.account.token_generation += 1;
    assert!(!same_registration(&changed, &record));
    let mut changed = record.clone();
    changed.account.created_at_ms += 1;
    assert!(!same_registration(&changed, &record));
    let mut changed = record.clone();
    changed.account.in_pool = true;
    assert!(!same_registration(&changed, &record));
}

#[test]
fn authorization_diagnostic_uses_current_item_overlays_without_rewriting_state() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(
            "CREATE TABLE app_state (key TEXT PRIMARY KEY, value_json TEXT NOT NULL);
         CREATE TABLE app_state_items (state_key TEXT, position INTEGER, value_json TEXT);
         INSERT INTO app_state VALUES ('accounts', '[{\"version\":1},{\"version\":2}]');
         INSERT INTO app_state_items VALUES ('accounts', 0, '{\"version\":3}');
         INSERT INTO app_state_items VALUES ('sources', 1, '{\"version\":99}');",
        )
        .unwrap();
    let before = connection.total_changes();
    assert_eq!(
        account_rows(&connection).unwrap(),
        ["{\"version\":3}", "{\"version\":2}"]
    );
    assert_eq!(connection.total_changes(), before);
    connection
        .execute(
            "UPDATE app_state SET value_json='[]' WHERE key='accounts'",
            [],
        )
        .unwrap();
    assert!(account_rows(&connection).unwrap().is_empty());
}

#[test]
fn authorization_diagnostic_rejects_a_missing_root_without_creating_it() {
    let root = std::env::temp_dir().join(format!("relay-auth-diagnostic-{}", uuid::Uuid::new_v4()));
    assert!(!root.exists());
    assert!(load_records(&root).is_err());
    assert!(!root.exists());
    let backend = DiagnosticSecrets(&root);
    assert!(
        backend
            .save("synthetic-reference", "synthetic-value")
            .is_err()
    );
    assert!(backend.delete("synthetic-reference").is_err());
    assert!(!root.exists());
}

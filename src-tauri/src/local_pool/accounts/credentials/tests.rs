use super::super::import_session::{SecretBackend, SecretBackendError};
use super::*;
use std::sync::Arc;
use std::{collections::HashMap, sync::Mutex};
use zenith_relay_core::accounts::TokenSet;
use zenith_relay_core::providers::chatgpt::AgentIdentityCredential;

const ACCESS: &str = "stored-access-secret";
const REFRESH: &str = "stored-refresh-secret";
const ID_TOKEN: &str = "stored-id-secret";
const EMAIL: &str = "stored.user@example.test";
const PROVIDER_ACCOUNT: &str = "provider-account-secret-id";
const PROXY: &str = "http://proxy-user:proxy-pass@proxy.example:8080/";

#[derive(Default)]
struct MemorySecrets(Mutex<HashMap<String, String>>);

impl SecretBackend for MemorySecrets {
    fn save(&self, secret_ref: &str, value: &str) -> Result<(), SecretBackendError> {
        self.0
            .lock()
            .unwrap()
            .insert(secret_ref.into(), value.into());
        Ok(())
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>, SecretBackendError> {
        Ok(self.0.lock().unwrap().get(secret_ref).cloned())
    }

    fn delete(&self, secret_ref: &str) -> Result<(), SecretBackendError> {
        self.0.lock().unwrap().remove(secret_ref);
        Ok(())
    }
}

#[test]
fn debug_and_snapshot_redact_tokens_email_and_provider_ids() {
    let credentials = fixture();
    let debug = format!("{credentials:?}");
    let snapshot = serde_json::to_string(&credentials.snapshot()).unwrap();
    for secret in [ACCESS, REFRESH, ID_TOKEN, EMAIL, PROVIDER_ACCOUNT, PROXY] {
        assert!(!debug.contains(secret));
        assert!(!snapshot.contains(secret));
    }
    assert!(debug.contains("[redacted]"));
    assert!(snapshot.contains("s***@e***.test"));
}

#[test]
fn bearer_authorization_is_sensitive_and_uses_the_canonical_scheme() {
    let authorization = bearer_authorization("synthetic-access-token").unwrap();
    assert_eq!(
        authorization.to_str().unwrap(),
        "Bearer synthetic-access-token"
    );
    assert!(authorization.is_sensitive());
}

#[test]
fn native_secret_json_round_trips_after_restart() {
    let backend = Arc::new(MemorySecrets::default());
    let first = CredentialStore::new(backend.clone());
    first.save(&fixture()).unwrap();
    drop(first);

    let reopened = CredentialStore::new(backend);
    let loaded = reopened.require("relay_account_1").unwrap();
    assert_eq!(loaded.access_token(), ACCESS);
    assert_eq!(loaded.refresh_token(), Some(REFRESH));
    assert_eq!(loaded.id_token(), Some(ID_TOKEN));
    assert_eq!(loaded.provider_account_id(), Some(PROVIDER_ACCOUNT));
    assert_eq!(loaded.proxy_url(), Some(PROXY));
    assert_eq!(loaded.generation(), 7);
    assert_eq!(
        credential_secret_ref("relay_account_1").unwrap(),
        "account:codex:relay_account_1"
    );
}

#[test]
fn token_set_merge_preserves_private_identity_metadata() {
    let credentials = fixture();
    let tokens = TokenSet::new("new-access", None, None, Some(9_000), 2_000, 8).unwrap();
    let updated = credentials.with_token_set(&tokens).unwrap();
    assert_eq!(updated.access_token(), "new-access");
    assert_eq!(updated.refresh_token(), Some(REFRESH));
    assert_eq!(updated.id_token(), Some(ID_TOKEN));
    assert_eq!(updated.provider_account_id(), Some(PROVIDER_ACCOUNT));
    assert_eq!(updated.proxy_url(), Some(PROXY));
    assert_eq!(updated.generation(), 8);
}

#[test]
fn credential_refresh_preserves_private_identity_metadata() {
    let credentials = fixture();
    let refresh = CredentialRefresh::new("new-access".into(), None, None, Some(9_000)).unwrap();

    let updated = credentials.apply_refresh(refresh, 2_000).unwrap();

    assert_eq!(updated.access_token(), "new-access");
    assert_eq!(updated.refresh_token(), Some(REFRESH));
    assert_eq!(updated.id_token(), Some(ID_TOKEN));
    assert_eq!(updated.provider_account_id(), Some(PROVIDER_ACCOUNT));
    assert_eq!(updated.proxy_url(), Some(PROXY));
    assert_eq!(updated.issued_at_ms(), 2_000);
    assert_eq!(updated.generation(), 8);
}

#[test]
fn direct_route_survives_storage_and_token_refresh() {
    let backend = Arc::new(MemorySecrets::default());
    let store = CredentialStore::new(backend);
    let direct = fixture().with_proxy_route(None, true).unwrap();
    store.save(&direct).unwrap();

    let loaded = store.require("relay_account_1").unwrap();
    assert!(loaded.bypass_common_proxy());
    let tokens = TokenSet::new("new-access", None, None, Some(9_000), 2_000, 8).unwrap();
    assert!(loaded
        .with_token_set(&tokens)
        .unwrap()
        .bypass_common_proxy());
}

#[test]
fn agent_identity_round_trips_without_becoming_an_oauth_token() {
    const PRIVATE_KEY: &str = "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";
    let backend = Arc::new(MemorySecrets::default());
    let store = CredentialStore::new(backend);
    let credential = StoredCodexCredentials::new_agent_identity(
        "agent_account",
        AgentIdentityCredential::new(
            PRIVATE_KEY.into(),
            "runtime-test".into(),
            "task-test".into(),
        )
        .unwrap(),
        1,
        2,
        Some("agent@example.test".into()),
        Some("provider-agent".into()),
        None,
        None,
        Some("team".into()),
        false,
    )
    .unwrap();
    store.save(&credential).unwrap();

    let loaded = store.require("agent_account").unwrap();
    assert!(loaded.is_agent_identity());
    assert!(loaded.access_token().is_empty());
    assert!(loaded.to_token_set().is_err());
    assert!(loaded
        .authorization(1_785_000_000_000)
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("AgentAssertion "));
    assert!(!format!("{loaded:?}").contains(PRIVATE_KEY));
}

#[test]
fn oauth_fallback_round_trips_and_survives_token_refresh_with_agent_identity() {
    const PRIVATE_KEY: &str = "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";
    let backend = Arc::new(MemorySecrets::default());
    let store = CredentialStore::new(backend);
    let credentials = fixture().with_agent_identity(
        AgentIdentityCredential::new(
            PRIVATE_KEY.into(),
            "runtime-test".into(),
            "task-test".into(),
        )
        .unwrap(),
    );
    store.save(&credentials).unwrap();

    let loaded = store.require("relay_account_1").unwrap();
    assert!(loaded.is_agent_identity());
    assert!(loaded.has_oauth());
    assert_eq!(loaded.to_token_set().unwrap().access_token(), ACCESS);
    let refreshed = loaded
        .with_token_set(&TokenSet::new("new-access", None, None, Some(9_000), 2_000, 8).unwrap())
        .unwrap();
    assert_eq!(refreshed.access_token(), "new-access");
    assert_eq!(
        refreshed.agent_identity().unwrap().task_id(),
        Some("task-test")
    );
}

fn fixture() -> StoredCodexCredentials {
    StoredCodexCredentials::new(
        "relay_account_1",
        ACCESS.into(),
        Some(REFRESH.into()),
        Some(ID_TOKEN.into()),
        Some(1),
        0,
        7,
        Some(EMAIL.into()),
        Some(PROVIDER_ACCOUNT.into()),
        Some("provider-user-secret-id".into()),
        Some("provider-org-secret-id".into()),
        Some("plus".into()),
        false,
    )
    .unwrap()
    .with_proxy_url(Some(PROXY.into()))
    .unwrap()
}

#[test]
fn login_notes_round_trip_survive_refresh_and_stay_out_of_debug() {
    let credentials = fixture().apply_stored_login_material(
        Some("950000000".into()),
        Some("synthetic-password".into()),
        Some("GEZDGNBVGY3TQOJQ".into()),
    );
    let debug = format!("{credentials:?}");
    assert!(!debug.contains("synthetic-password"));
    assert!(!debug.contains("GEZDGNBVGY3TQOJQ"));
    assert!(!debug.contains("950000000"));
    let loaded =
        StoredCodexCredentials::from_secret_json(&credentials.to_secret_json().unwrap()).unwrap();
    assert_eq!(loaded.phone(), Some("950000000"));
    assert_eq!(loaded.password(), Some("synthetic-password"));
    assert_eq!(loaded.totp_secret(), Some("GEZDGNBVGY3TQOJQ"));
    let refreshed = loaded
        .with_token_set(&TokenSet::new("new-access", None, None, Some(9_000), 2_000, 8).unwrap())
        .unwrap();
    assert_eq!(refreshed.access_token(), "new-access");
    assert_eq!(refreshed.password(), Some("synthetic-password"));
    assert_eq!(refreshed.totp_secret(), Some("GEZDGNBVGY3TQOJQ"));
    let legacy =
        StoredCodexCredentials::from_secret_json(&fixture().to_secret_json().unwrap()).unwrap();
    assert_eq!(legacy.password(), None);
    assert_eq!(legacy.phone(), None);
    assert_eq!(legacy.totp_secret(), None);
}

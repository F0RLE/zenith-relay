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

fn siwc_fixture() -> StoredCodexCredentials {
    use crate::local_pool::accounts::oauth::{siwc::SiwcRegistration, OAuthClientKind};
    let registration = SiwcRegistration {
        client_id: "oaiapp_synthetic".into(),
        host_id: "urn:uuid:synthetic-host".into(),
        issuer: "https://auth.openai.com".into(),
        subject: "synthetic-subject".into(),
        scopes: vec!["resource.invoke".into(), "chatgpt.tokens.use.direct".into()],
        earliest_refresh_at_ms: None,
        requires_reauthentication: false,
    };
    StoredCodexCredentials::new(
        "siwc_synthetic",
        ACCESS.into(),
        Some(REFRESH.into()),
        Some(ID_TOKEN.into()),
        Some(10),
        1,
        1,
        Some(EMAIL.into()),
        Some(registration.identity_key()),
        Some(registration.subject.clone()),
        None,
        None,
        false,
    )
    .unwrap()
    .with_oauth_client_kind(OAuthClientKind::Siwc)
    .with_siwc_registration(Some(registration))
    .unwrap()
}

#[test]
fn siwc_rotation_recovers_after_restart_without_reusing_the_old_grant() {
    let backend = Arc::new(MemorySecrets::default());
    let store = CredentialStore::new(backend.clone());
    let before = siwc_fixture();
    let after = before
        .with_token_set(
            &TokenSet::new(
                "rotated-access",
                Some("rotated-refresh".into()),
                Some(ID_TOKEN.into()),
                Some(1_000_000),
                10,
                2,
            )
            .unwrap(),
        )
        .unwrap();
    store.save(&before).unwrap();
    store.checkpoint_siwc_rotation(&before, None).unwrap();
    assert!(store.recover_siwc_rotation(&before).is_err());
    assert!(store
        .require("siwc_synthetic")
        .unwrap()
        .matches_snapshot(&before));
    store
        .checkpoint_siwc_rotation(&before, Some(&after))
        .unwrap();
    drop(store);
    let restarted = CredentialStore::new(backend);
    let recovered = restarted.recover_siwc_rotation(&before).unwrap().unwrap();
    assert!(recovered.matches_snapshot(&after));
    assert!(restarted
        .require("siwc_synthetic")
        .unwrap()
        .matches_snapshot(&after));
    assert!(restarted.recover_siwc_rotation(&after).unwrap().is_none());
    assert_eq!(recovered.siwc_registration(), before.siwc_registration());
}

#[test]
fn siwc_rotation_cannot_overwrite_a_newer_sign_in_or_survive_deletion() {
    let store = CredentialStore::from_backend(MemorySecrets::default());
    let before = siwc_fixture();
    store.save(&before).unwrap();
    store.checkpoint_siwc_rotation(&before, None).unwrap();
    let newer = before
        .with_token_set(
            &TokenSet::new(
                "new-login-access",
                Some("new-login-refresh".into()),
                Some(ID_TOKEN.into()),
                Some(1_000_000),
                20,
                3,
            )
            .unwrap(),
        )
        .unwrap();
    store.save(&newer).unwrap();
    assert!(store.recover_siwc_rotation(&newer).unwrap().is_none());
    assert!(store
        .require("siwc_synthetic")
        .unwrap()
        .matches_snapshot(&newer));
    store.checkpoint_siwc_rotation(&newer, None).unwrap();
    store.delete("siwc_synthetic").unwrap();
    assert!(store.load("siwc_synthetic").unwrap().is_none());
    assert!(store.recover_siwc_rotation(&newer).unwrap().is_none());
}

#[test]
fn siwc_rotation_survives_proxy_changes_without_restoring_old_settings() {
    let store = CredentialStore::from_backend(MemorySecrets::default());
    let before = siwc_fixture();
    store.save(&before).unwrap();
    store.checkpoint_siwc_rotation(&before, None).unwrap();
    let changed = before.clone().with_proxy_url(Some(PROXY.into())).unwrap();
    store.save(&changed).unwrap();
    assert!(store.recover_siwc_rotation(&changed).is_err());
    assert!(store.recover_siwc_rotation(&changed).is_err());
    let rotated = before
        .with_token_set(
            &TokenSet::new(
                "rotated-access",
                Some("rotated-refresh".into()),
                Some(ID_TOKEN.into()),
                Some(1_000_000),
                10,
                2,
            )
            .unwrap(),
        )
        .unwrap();
    store
        .checkpoint_siwc_rotation(&before, Some(&rotated))
        .unwrap();
    let recovered = store.recover_siwc_rotation(&changed).unwrap().unwrap();
    assert_eq!(recovered.refresh_token(), Some("rotated-refresh"));
    assert_eq!(recovered.proxy_url(), Some(PROXY));
    assert_eq!(recovered.generation(), 2);
    assert!(store
        .require("siwc_synthetic")
        .unwrap()
        .matches_snapshot(&recovered));
}

#[derive(Default)]
struct MemorySecrets(
    Mutex<HashMap<String, String>>,
    std::sync::atomic::AtomicBool,
);

impl SecretBackend for MemorySecrets {
    fn save(&self, secret_ref: &str, secret_value: &str) -> Result<(), SecretBackendError> {
        self.0
            .lock()
            .unwrap()
            .insert(secret_ref.into(), secret_value.into());
        Ok(())
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>, SecretBackendError> {
        Ok(self.0.lock().unwrap().get(secret_ref).cloned())
    }

    fn delete(&self, secret_ref: &str) -> Result<(), SecretBackendError> {
        if self.1.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(SecretBackendError);
        }
        self.0.lock().unwrap().remove(secret_ref);
        Ok(())
    }
}

#[test]
fn failed_primary_delete_retains_the_siwc_rotation_barrier() {
    let backend = Arc::new(MemorySecrets::default());
    let store = CredentialStore::new(backend.clone());
    let before = siwc_fixture();
    store.save(&before).unwrap();
    store.checkpoint_siwc_rotation(&before, None).unwrap();
    backend.1.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(store.delete(before.local_account_id()).is_err());
    assert!(store
        .require(before.local_account_id())
        .unwrap()
        .matches_snapshot(&before));
    assert!(store.recover_siwc_rotation(&before).is_err());
    backend.1.store(false, std::sync::atomic::Ordering::SeqCst);
    store.delete(before.local_account_id()).unwrap();
    assert!(store.load(before.local_account_id()).unwrap().is_none());
    assert!(backend.0.lock().unwrap().is_empty());
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
fn missing_stored_expiry_is_recovered_only_from_the_current_access_token() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let token = format!(
        "header.{}.signature",
        URL_SAFE_NO_PAD.encode(serde_json::json!({"exp": 2_000}).to_string()),
    );
    let mut secret: serde_json::Value =
        serde_json::from_str(&fixture().to_secret_json().unwrap()).unwrap();
    secret["accessToken"] = token.into();
    secret["expiresAtMs"] = serde_json::Value::Null;
    let restored = StoredCodexCredentials::from_secret_json(&secret.to_string()).unwrap();
    assert_eq!(restored.expires_at_ms(), Some(2_000_000));
    assert_eq!(
        restored.to_token_set().unwrap().expires_at_ms(),
        Some(2_000_000)
    );
    assert_eq!(restored.generation(), fixture().generation());
    secret["expiresAtMs"] = 3_000_000.into();
    let explicit = StoredCodexCredentials::from_secret_json(&secret.to_string()).unwrap();
    assert_eq!(explicit.expires_at_ms(), Some(3_000_000));
    let rotated = restored
        .with_token_set(&TokenSet::new("opaque-rotated-token", None, None, None, 1_000, 8).unwrap())
        .unwrap();
    assert_eq!(rotated.expires_at_ms(), None);
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

#[test]
fn oauth_reauth_preserves_local_login_notes_when_provider_email_changes() {
    let previous = fixture().apply_stored_login_material(
        Some("950000000".into()),
        Some("synthetic-password".into()),
        Some("GEZDGNBVGY3TQOJQ".into()),
    );
    let replacement = StoredCodexCredentials::new(
        "relay_account_1",
        "replacement-access".into(),
        Some("replacement-refresh".into()),
        Some("replacement-id".into()),
        Some(10_000),
        9_000,
        8,
        Some("new-provider@example.test".into()),
        Some(PROVIDER_ACCOUNT.into()),
        None,
        None,
        Some("plus".into()),
        false,
    )
    .unwrap();

    let merged = replacement.fill_missing_login_from(&previous);
    assert_eq!(merged.email(), Some("new-provider@example.test"));
    assert_eq!(merged.phone(), Some("950000000"));
    assert_eq!(merged.password(), Some("synthetic-password"));
    assert_eq!(merged.totp_secret(), Some("GEZDGNBVGY3TQOJQ"));
}

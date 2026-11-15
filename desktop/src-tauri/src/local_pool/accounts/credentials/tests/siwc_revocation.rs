use super::*;
use crate::local_pool::accounts::oauth::siwc::PendingSiwcRefresh;
use serde_json::json;
use std::sync::atomic::Ordering;

fn pending(before: &StoredCodexCredentials) -> PendingSiwcRefresh {
    serde_json::from_value(json!({
        "registration":before.siwc_registration(),"access_token":"received-access",
        "refresh_token":"received-refresh","id_token":"unverified-id",
        "expires_at_ms":3_600_010,"received_at_ms":10
    }))
    .unwrap()
}

fn replacement(before: &StoredCodexCredentials) -> StoredCodexCredentials {
    before
        .with_token_set(
            &TokenSet::new(
                "received-access",
                Some("received-refresh".into()),
                Some(ID_TOKEN.into()),
                Some(3_600_010),
                10,
                before.generation() + 1,
            )
            .unwrap(),
        )
        .unwrap()
}

#[test]
fn siwc_revocation_uses_latest_received_token_without_activating_it() {
    for verified in [false, true] {
        let backend = Arc::new(MemorySecrets::default());
        let store = CredentialStore::new(backend.clone());
        let before = siwc_fixture();
        store.save(&before).unwrap();
        if verified {
            store
                .checkpoint_token_rotation(&before, Some(&replacement(&before)))
                .unwrap();
        } else {
            store
                .checkpoint_pending_siwc_rotation(&before, &pending(&before))
                .unwrap();
        }
        let snapshot = backend.0.lock().unwrap().clone();
        assert_eq!(
            store.siwc_revocation_token(&before).unwrap().as_deref(),
            Some("received-refresh")
        );
        assert_eq!(*backend.0.lock().unwrap(), snapshot);
        assert!(
            store
                .require(before.local_account_id())
                .unwrap()
                .matches_snapshot(&before)
        );
    }
}

#[test]
fn siwc_revocation_requires_a_known_rotation_outcome() {
    let store = CredentialStore::from_backend(MemorySecrets::default());
    let before = siwc_fixture();
    store.save(&before).unwrap();
    assert_eq!(
        store.siwc_revocation_token(&before).unwrap().as_deref(),
        Some(REFRESH)
    );
    store.checkpoint_token_rotation(&before, None).unwrap();
    assert_eq!(
        store.siwc_revocation_token(&before).unwrap_err().code,
        CredentialErrorCode::RotationOutcomeUnknown
    );
    assert!(
        store
            .require(before.local_account_id())
            .unwrap()
            .matches_snapshot(&before)
    );
}

#[test]
fn siwc_identity_only_and_unactivated_imports_have_no_session_to_revoke() {
    let store = CredentialStore::from_backend(MemorySecrets::default());
    let before = siwc_fixture();
    let mut registration = before.siwc_registration().unwrap().clone();
    registration.requires_reauthentication = true;
    let imported = before
        .with_siwc_registration(Some(registration.clone()))
        .unwrap();
    assert_eq!(store.siwc_revocation_token(&imported).unwrap(), None);
    registration.requires_reauthentication = false;
    registration.scopes = vec!["openid".into(), "email".into()];
    let identity = StoredCodexCredentials::new_siwc_identity(
        "siwc_identity",
        registration,
        ID_TOKEN.into(),
        None,
        1,
        1,
    )
    .unwrap();
    assert_eq!(store.siwc_revocation_token(&identity).unwrap(), None);
}

#[test]
fn siwc_revocation_rejects_unreadable_or_mismatched_checkpoints() {
    let backend = Arc::new(MemorySecrets::default());
    let store = CredentialStore::new(backend.clone());
    let before = siwc_fixture();
    store.save(&before).unwrap();
    backend.2.store(true, Ordering::SeqCst);
    assert_eq!(
        store.siwc_revocation_token(&before).unwrap_err().code,
        CredentialErrorCode::SecretStoreUnavailable
    );
    backend.2.store(false, Ordering::SeqCst);
    let mismatched = replacement(&replacement(&before));
    store
        .checkpoint_token_rotation(&before, Some(&mismatched))
        .unwrap();
    assert_eq!(
        store.siwc_revocation_token(&before).unwrap_err().code,
        CredentialErrorCode::RotationCheckpointInvalid
    );
}

#[test]
fn siwc_revocation_ignores_a_checkpoint_superseded_by_new_credentials() {
    let store = CredentialStore::from_backend(MemorySecrets::default());
    let before = siwc_fixture();
    store
        .checkpoint_pending_siwc_rotation(&before, &pending(&before))
        .unwrap();
    let newer = before
        .with_token_set(
            &TokenSet::new(
                "new-login-access",
                Some("new-login-refresh".into()),
                Some(ID_TOKEN.into()),
                Some(3_600_010),
                10,
                before.generation() + 1,
            )
            .unwrap(),
        )
        .unwrap();
    store.save(&newer).unwrap();
    assert_eq!(
        store.siwc_revocation_token(&newer).unwrap().as_deref(),
        Some("new-login-refresh")
    );
    assert!(store.pending_siwc_rotation(&newer).unwrap().is_none());
}

#[test]
fn siwc_revocation_rejects_a_pending_replacement_for_another_registration() {
    let backend = Arc::new(MemorySecrets::default());
    let store = CredentialStore::new(backend.clone());
    let before = siwc_fixture();
    let mut received = serde_json::to_value(pending(&before)).unwrap();
    received["registration"]["clientId"] = json!("oaiapp_other");
    let reference = format!(
        "{}:siwc-rotation",
        credential_secret_ref(before.local_account_id()).unwrap()
    );
    let checkpoint = json!({
        "before":before.to_secret_json().unwrap(),"after":null,"pending_siwc":received
    });
    backend.save(&reference, &checkpoint.to_string()).unwrap();
    assert_eq!(
        store.siwc_revocation_token(&before).unwrap_err().code,
        CredentialErrorCode::RotationCheckpointInvalid
    );
    assert_eq!(
        backend.load(&reference).unwrap(),
        Some(checkpoint.to_string())
    );
}

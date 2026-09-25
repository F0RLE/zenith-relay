use super::{runtime_error, store_error, vault_error, ManagementError};
use crate::state::{
    generate_pool_key, now_ms, AppState, GatewayKeyRecord, PROFILE_KEY_ROTATION_PREFIX,
    SYSTEM_GATEWAY_KEY_ID,
};
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use std::sync::Arc;
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::{ProfileKeyRotation, PROFILE_KEY_ROTATION_SCHEMA_VERSION};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/profile/credential", get(profile_credential))
        .route(
            "/profile/credential/rotations",
            post(prepare_profile_key_rotation),
        )
        .route(
            "/profile/credential/rotations/{id}",
            post(commit_profile_key_rotation).delete(abort_profile_key_rotation),
        )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileCredential {
    key_id: String,
    base_url: String,
    secret: String,
}

pub async fn profile_credential(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ManagementError> {
    let _configuration = state.configuration_lock.lock().await;
    let base_url = profile_gateway_base_url(&state)?;
    let mut key = state
        .store
        .keys()
        .map_err(store_error)?
        .into_iter()
        .find(|key| key.id == SYSTEM_GATEWAY_KEY_ID)
        .ok_or_else(|| {
            ManagementError::internal(
                error_codes::SYSTEM_KEY_MISSING,
                "managed profile credential is unavailable",
            )
        })?;
    let secret = state
        .vault
        .load(&key.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::internal(
                error_codes::SYSTEM_KEY_MISSING,
                "managed profile credential is unavailable",
            )
        })?;
    if !key.enabled {
        let build = state.lock_runtime_rebuild().await;
        let old = key.clone();
        key.enabled = true;
        state.store.save_key(&key).map_err(store_error)?;
        build
            .rebuild_or_rollback(&state, || state.store.save_key(&old))
            .await
            .map_err(runtime_error)?;
    }
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(ProfileCredential {
            key_id: key.id,
            base_url,
            secret,
        }),
    ))
}

pub async fn prepare_profile_key_rotation(
    State(state): State<Arc<AppState>>,
) -> Result<impl IntoResponse, ManagementError> {
    let _configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let base_url = profile_gateway_base_url(&state)?;
    let current = state
        .store
        .keys()
        .map_err(store_error)?
        .into_iter()
        .find(|key| key.id == SYSTEM_GATEWAY_KEY_ID)
        .ok_or_else(|| {
            ManagementError::internal(
                error_codes::SYSTEM_KEY_MISSING,
                "managed profile credential is unavailable",
            )
        })?;
    let rotation_id = format!(
        "{PROFILE_KEY_ROTATION_PREFIX}{}",
        uuid::Uuid::new_v4().simple()
    );
    let secret_ref = format!("key:{rotation_id}");
    let secret = generate_pool_key();
    let mut pending = current;
    pending.id = rotation_id.clone();
    pending.label = "ChatGPT pending rotation".to_string();
    pending.enabled = true;
    pending.secret_ref = secret_ref.clone();
    pending.created_at_ms = now_ms();
    pending.last_used_at_ms = None;
    state
        .vault
        .save(&secret_ref, &secret)
        .map_err(vault_error)?;
    if let Err(error) = state.store.save_key(&pending) {
        let _ = state.vault.delete(&secret_ref);
        return Err(store_error(error));
    }
    build
        .rebuild_or_rollback(&state, || {
            state.store.delete_key(&rotation_id)?;
            state.vault.delete(&secret_ref)?;
            Ok(())
        })
        .await
        .map_err(runtime_error)?;
    Ok((
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(ProfileKeyRotation {
            schema_version: PROFILE_KEY_ROTATION_SCHEMA_VERSION,
            rotation_id,
            key_id: SYSTEM_GATEWAY_KEY_ID.to_string(),
            base_url,
            secret,
        }),
    ))
}

pub async fn commit_profile_key_rotation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    validate_profile_rotation_id(&id)?;
    let _configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let keys = state.store.keys().map_err(store_error)?;
    let current = keys
        .iter()
        .find(|key| key.id == SYSTEM_GATEWAY_KEY_ID)
        .cloned()
        .ok_or_else(|| {
            ManagementError::internal(
                error_codes::SYSTEM_KEY_MISSING,
                "managed profile credential is unavailable",
            )
        })?;
    let rotations = keys
        .into_iter()
        .filter(|key| key.system && key.id.starts_with(PROFILE_KEY_ROTATION_PREFIX))
        .map(|key| {
            let secret = state.vault.load(&key.secret_ref).map_err(vault_error)?;
            Ok((key, secret))
        })
        .collect::<Result<Vec<_>, ManagementError>>()?;
    let new_secret = rotations
        .iter()
        .find(|(key, _)| key.id == id)
        .and_then(|(_, secret)| secret.clone())
        .ok_or_else(|| {
            ManagementError::not_found(
                error_codes::PROFILE_ROTATION_MISSING,
                "profile credential rotation was not found",
            )
        })?;
    let old_secret = state
        .vault
        .load(&current.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::internal(
                error_codes::SYSTEM_KEY_MISSING,
                "managed profile credential is unavailable",
            )
        })?;
    // Both the old profile key and every pending rotation belong to the old
    // runtime. Retire it before changing the vault so an already-admitted
    // request cannot dispatch under a revoked key during the rebuild window.
    state.replace_runtime(None).map_err(runtime_error)?;
    if let Err(error) = state.vault.save(&current.secret_ref, &new_secret) {
        build
            .rollback_and_rebuild(&state, || {
                restore_profile_rotation(&state, &current, &old_secret, &rotations)
            })
            .await
            .map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
        return Err(vault_error(error));
    }
    for (key, _) in &rotations {
        if let Err(error) = state.store.delete_key(&key.id) {
            build
                .rollback_and_rebuild(&state, || {
                    restore_profile_rotation(&state, &current, &old_secret, &rotations)
                })
                .await
                .map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
            return Err(store_error(error));
        }
        if let Err(error) = state.vault.delete(&key.secret_ref) {
            build
                .rollback_and_rebuild(&state, || {
                    restore_profile_rotation(&state, &current, &old_secret, &rotations)
                })
                .await
                .map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
            return Err(vault_error(error));
        }
    }
    build
        .rebuild_or_rollback(&state, || {
            restore_profile_rotation(&state, &current, &old_secret, &rotations)
        })
        .await
        .map_err(runtime_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn abort_profile_key_rotation(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    validate_profile_rotation_id(&id)?;
    let _configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let key = state
        .store
        .keys()
        .map_err(store_error)?
        .into_iter()
        .find(|key| key.system && key.id == id)
        .ok_or_else(|| {
            ManagementError::not_found(
                error_codes::PROFILE_ROTATION_MISSING,
                "profile credential rotation was not found",
            )
        })?;
    let secret = state.vault.load(&key.secret_ref).map_err(vault_error)?;
    state.replace_runtime(None).map_err(runtime_error)?;
    if let Err(error) = state.store.delete_key(&id) {
        build
            .rollback_and_rebuild(&state, || {
                restore_pending_profile_rotation(&state, &key, secret.as_deref())
            })
            .await
            .map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
        return Err(store_error(error));
    }
    if let Err(error) = state.vault.delete(&key.secret_ref) {
        build
            .rollback_and_rebuild(&state, || {
                restore_pending_profile_rotation(&state, &key, secret.as_deref())
            })
            .await
            .map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
        return Err(vault_error(error));
    }
    build
        .rebuild_or_rollback(&state, || {
            restore_pending_profile_rotation(&state, &key, secret.as_deref())
        })
        .await
        .map_err(runtime_error)?;
    Ok(StatusCode::NO_CONTENT)
}

fn profile_gateway_base_url(state: &AppState) -> Result<String, ManagementError> {
    let snapshot = state.snapshot().map_err(store_error)?;
    if !state.store.gateway_enabled().map_err(store_error)? {
        return Err(ManagementError::new(
            StatusCode::CONFLICT,
            error_codes::PROFILE_ATTACH_UNAVAILABLE,
            "remote gateway is stopped",
            "profile_attach",
            true,
        ));
    }
    Ok(snapshot.gateway.base_url)
}

fn validate_profile_rotation_id(id: &str) -> Result<(), ManagementError> {
    if id.len() <= PROFILE_KEY_ROTATION_PREFIX.len()
        || id.len() > 128
        || !id.starts_with(PROFILE_KEY_ROTATION_PREFIX)
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ManagementError::validation(
            error_codes::PROFILE_ROTATION_INVALID,
            "profile credential rotation ID is invalid",
        ));
    }
    Ok(())
}
fn restore_profile_rotation(
    state: &AppState,
    current: &GatewayKeyRecord,
    current_secret: &str,
    rotations: &[(GatewayKeyRecord, Option<String>)],
) -> Result<(), String> {
    state.vault.save(&current.secret_ref, current_secret)?;
    for (key, secret) in rotations {
        if let Some(secret) = secret.as_deref() {
            state.vault.save(&key.secret_ref, secret)?;
        }
        state.store.save_key(key)?;
    }
    Ok(())
}

fn restore_pending_profile_rotation(
    state: &AppState,
    key: &GatewayKeyRecord,
    secret: Option<&str>,
) -> Result<(), String> {
    if let Some(secret) = secret {
        state.vault.save(&key.secret_ref, secret)?;
    }
    state.store.save_key(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        store::{Store, Vault},
        test_fixtures::pooled_source,
    };
    use tempfile::TempDir;

    async fn fixture() -> (TempDir, Arc<AppState>, GatewayKeyRecord, GatewayKeyRecord) {
        let root = TempDir::new().unwrap();
        let config = Config::for_test(root.path().into(), "127.0.0.1:0".parse().unwrap());
        let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
        let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
        let state = AppState::new(config, store, vault).unwrap();
        let source = pooled_source("key-source", "test-model");
        state.store.save_source(&source).unwrap();
        state
            .vault
            .save(&source.secret_ref, "synthetic-source-key")
            .unwrap();
        let current = state
            .store
            .keys()
            .unwrap()
            .into_iter()
            .find(|key| key.id == SYSTEM_GATEWAY_KEY_ID)
            .unwrap();
        state
            .vault
            .save(&current.secret_ref, "synthetic-old-key")
            .unwrap();
        let pending = GatewayKeyRecord {
            id: format!("{PROFILE_KEY_ROTATION_PREFIX}synthetic"),
            label: "Synthetic pending key".into(),
            enabled: true,
            system: true,
            secret_ref: format!("key:{PROFILE_KEY_ROTATION_PREFIX}synthetic"),
            created_at_ms: now_ms(),
            last_used_at_ms: None,
        };
        state
            .vault
            .save(&pending.secret_ref, "synthetic-new-key")
            .unwrap();
        state.store.save_key(&pending).unwrap();
        state.rebuild_runtime().await.unwrap();
        (root, state, current, pending)
    }

    #[tokio::test]
    async fn profile_key_commit_waits_for_build_before_changing_live_secret() {
        let (_root, state, current, pending) = fixture().await;
        let old_runtime = state.runtime().unwrap().unwrap();
        let old_build = state.lock_runtime_rebuild().await;
        let worker_state = state.clone();
        let rotation_id = pending.id.clone();
        let commit = tokio::spawn(async move {
            commit_profile_key_rotation(State(worker_state), Path(rotation_id)).await
        });
        tokio::task::yield_now().await;
        assert!(!commit.is_finished());
        assert_eq!(
            state.vault.load(&current.secret_ref).unwrap().as_deref(),
            Some("synthetic-old-key")
        );
        assert!(state
            .store
            .keys()
            .unwrap()
            .iter()
            .any(|key| key.id == pending.id));
        drop(old_build);

        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), commit)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            StatusCode::NO_CONTENT
        );
        assert_eq!(
            state.vault.load(&current.secret_ref).unwrap().as_deref(),
            Some("synthetic-new-key")
        );
        assert!(!state
            .store
            .keys()
            .unwrap()
            .iter()
            .any(|key| key.id == pending.id));
        assert!(old_runtime
            .candidate_runtime_order()
            .iter()
            .all(|candidate| !candidate.available));
        assert!(!Arc::ptr_eq(
            &old_runtime,
            &state.runtime().unwrap().unwrap()
        ));
        state.shutdown_runtime().await.unwrap();
    }

    #[tokio::test]
    async fn profile_key_abort_waits_for_build_before_removing_pending_secret() {
        let (_root, state, current, pending) = fixture().await;
        let old_runtime = state.runtime().unwrap().unwrap();
        let old_build = state.lock_runtime_rebuild().await;
        let worker_state = state.clone();
        let rotation_id = pending.id.clone();
        let abort = tokio::spawn(async move {
            abort_profile_key_rotation(State(worker_state), Path(rotation_id)).await
        });
        tokio::task::yield_now().await;
        assert!(!abort.is_finished());
        assert_eq!(
            state.vault.load(&pending.secret_ref).unwrap().as_deref(),
            Some("synthetic-new-key")
        );
        assert!(state
            .store
            .keys()
            .unwrap()
            .iter()
            .any(|key| key.id == pending.id));
        drop(old_build);

        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(5), abort)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            StatusCode::NO_CONTENT
        );
        assert!(state.vault.load(&pending.secret_ref).unwrap().is_none());
        assert!(!state
            .store
            .keys()
            .unwrap()
            .iter()
            .any(|key| key.id == pending.id));
        assert_eq!(
            state.vault.load(&current.secret_ref).unwrap().as_deref(),
            Some("synthetic-old-key")
        );
        assert!(old_runtime
            .candidate_runtime_order()
            .iter()
            .all(|candidate| !candidate.available));
        assert!(!Arc::ptr_eq(
            &old_runtime,
            &state.runtime().unwrap().unwrap()
        ));
        state.shutdown_runtime().await.unwrap();
    }

    #[tokio::test]
    async fn failed_profile_key_restore_keeps_the_old_runtime_retired() {
        let (root, state, current, pending) = fixture().await;
        let old_runtime = state.runtime().unwrap().unwrap();
        // An atomic vault write cannot complete while the backup destination
        // is a directory. Both commit and restoration then fail closed.
        let backup = root.path().join("vault/secrets.enc.bak");
        if backup.is_file() {
            std::fs::remove_file(&backup).unwrap();
        }
        std::fs::create_dir(&backup).unwrap();
        assert!(
            commit_profile_key_rotation(State(state.clone()), Path(pending.id.clone()))
                .await
                .is_err()
        );
        assert!(state.runtime().unwrap().is_none());
        assert!(old_runtime
            .candidate_runtime_order()
            .iter()
            .all(|candidate| !candidate.available));
        assert_eq!(
            state.vault.load(&current.secret_ref).unwrap().as_deref(),
            Some("synthetic-old-key")
        );
        assert!(state
            .store
            .keys()
            .unwrap()
            .iter()
            .any(|key| key.id == pending.id));
        std::fs::remove_dir(&backup).unwrap();
        state.shutdown_runtime().await.unwrap();
    }
}

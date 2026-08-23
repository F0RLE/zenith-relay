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
    let base_url = profile_gateway_base_url(&state)?;
    let mut key = state
        .store
        .keys()
        .map_err(store_error)?
        .into_iter()
        .find(|key| key.id == SYSTEM_GATEWAY_KEY_ID)
        .ok_or_else(|| {
            ManagementError::internal(
                "system_key_missing",
                "managed profile credential is unavailable",
            )
        })?;
    let secret = state
        .vault
        .load(&key.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::internal(
                "system_key_missing",
                "managed profile credential is unavailable",
            )
        })?;
    if !key.enabled {
        let old = key.clone();
        key.enabled = true;
        state.store.save_key(&key).map_err(store_error)?;
        state
            .rebuild_runtime_or_rollback(|| state.store.save_key(&old))
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
    let base_url = profile_gateway_base_url(&state)?;
    let current = state
        .store
        .keys()
        .map_err(store_error)?
        .into_iter()
        .find(|key| key.id == SYSTEM_GATEWAY_KEY_ID)
        .ok_or_else(|| {
            ManagementError::internal(
                "system_key_missing",
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
    state
        .rebuild_runtime_or_rollback(|| {
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
    let keys = state.store.keys().map_err(store_error)?;
    let current = keys
        .iter()
        .find(|key| key.id == SYSTEM_GATEWAY_KEY_ID)
        .cloned()
        .ok_or_else(|| {
            ManagementError::internal(
                "system_key_missing",
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
                "profile_rotation_missing",
                "profile credential rotation was not found",
            )
        })?;
    let old_secret = state
        .vault
        .load(&current.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::internal(
                "system_key_missing",
                "managed profile credential is unavailable",
            )
        })?;
    state
        .vault
        .save(&current.secret_ref, &new_secret)
        .map_err(vault_error)?;
    for (key, _) in &rotations {
        if let Err(error) = state.store.delete_key(&key.id) {
            state
                .rollback_and_rebuild_runtime(|| {
                    restore_profile_rotation(&state, &current, &old_secret, &rotations)
                })
                .await
                .map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
            return Err(store_error(error));
        }
        if let Err(error) = state.vault.delete(&key.secret_ref) {
            state
                .rollback_and_rebuild_runtime(|| {
                    restore_profile_rotation(&state, &current, &old_secret, &rotations)
                })
                .await
                .map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
            return Err(vault_error(error));
        }
    }
    state
        .rebuild_runtime_or_rollback(|| {
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
    let key = state
        .store
        .keys()
        .map_err(store_error)?
        .into_iter()
        .find(|key| key.system && key.id == id)
        .ok_or_else(|| {
            ManagementError::not_found(
                "profile_rotation_missing",
                "profile credential rotation was not found",
            )
        })?;
    let secret = state.vault.load(&key.secret_ref).map_err(vault_error)?;
    state.store.delete_key(&id).map_err(store_error)?;
    if let Err(error) = state.vault.delete(&key.secret_ref) {
        let _ = state.store.save_key(&key);
        return Err(vault_error(error));
    }
    state
        .rebuild_runtime_or_rollback(|| {
            if let Some(secret) = secret.as_deref() {
                state.vault.save(&key.secret_ref, secret)?;
            }
            state.store.save_key(&key)
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
            "profile_attach_unavailable",
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
            "profile_rotation_invalid",
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

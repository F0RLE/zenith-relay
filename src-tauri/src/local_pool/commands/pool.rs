use super::{restart_after_secret_change, sync_gateway_or_rollback, sync_records_or_rollback};
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
    models::{LocalGatewayKeyRecord, LocalPoolSnapshot},
    state::DesktopState,
    store::secret_store,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

type CommandResult<T> = std::result::Result<T, CommandError>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedLocalKey {
    pub key: LocalGatewayKeyRecord,
    pub secret: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateGatewayKeyInput {
    key_id: String,
    label: String,
    source_ids: Option<Vec<String>>,
    account_ids: Option<Vec<String>>,
    #[serde(default)]
    allowed_models: Vec<String>,
    #[serde(default)]
    excluded_models: Vec<String>,
    model_prefix: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRoutingInput {
    max_retry_candidates: u8,
    session_affinity: bool,
    session_affinity_ttl_seconds: u64,
}

#[tauri::command]
pub async fn create_local_gateway_key(
    label: String,
    source_ids: Option<Vec<String>>,
    account_ids: Option<Vec<String>>,
    allowed_models: Option<Vec<String>>,
    excluded_models: Option<Vec<String>>,
    model_prefix: Option<String>,
    state: State<'_, DesktopState>,
) -> CommandResult<GeneratedLocalKey> {
    let _mutation = state.setup_guard().await;
    let id = format!("key_{}", Uuid::new_v4().simple());
    let secret = format!("zlr_{}", Uuid::new_v4().simple());
    let secret_ref = format!("key:{id}");
    let mut record = LocalGatewayKeyRecord {
        id,
        label: if label.trim().is_empty() {
            "Default".into()
        } else {
            label
        },
        enabled: true,
        secret_ref: secret_ref.clone(),
        source_ids,
        account_ids,
        allowed_models: allowed_models.unwrap_or_default(),
        excluded_models: excluded_models.unwrap_or_default(),
        model_prefix,
        created_at: Utc::now().to_rfc3339(),
        last_used_at: None,
    };
    record.normalize();
    validate_key_record(&state, &record, true)?;
    let (old_sources, old_keys) = current_records(&state)?;
    secret_store::save(&secret_ref, &secret)?;
    if let Err(error) = state.store()?.upsert_key(record.clone()) {
        cleanup_created_secret(&secret_ref, &error)?;
        return Err(error.into());
    }
    if let Err(error) = sync_records_or_rollback(&state, old_sources, old_keys).await {
        let key_was_rolled_back = state.store()?.key(&record.id).is_none();
        if key_was_rolled_back {
            cleanup_created_secret(&secret_ref, &error)?;
        }
        return Err(error.into());
    }
    Ok(GeneratedLocalKey {
        key: record,
        secret,
    })
}

#[tauri::command]
pub async fn update_local_gateway_key(
    input: UpdateGatewayKeyInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let current = state
        .store()?
        .key(&input.key_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local key not found"))?;
    let mut updated = LocalGatewayKeyRecord {
        id: current.id,
        label: input.label,
        enabled: current.enabled,
        secret_ref: current.secret_ref,
        source_ids: input.source_ids,
        account_ids: input.account_ids,
        allowed_models: input.allowed_models,
        excluded_models: input.excluded_models,
        model_prefix: input.model_prefix,
        created_at: current.created_at,
        last_used_at: current.last_used_at,
    };
    updated.normalize();
    validate_key_record(&state, &updated, updated.enabled)?;
    let (old_sources, old_keys) = current_records(&state)?;
    state.store()?.upsert_key(updated)?;
    sync_records_or_rollback(&state, old_sources, old_keys).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_local_gateway_key_enabled(
    key_id: String,
    enabled: bool,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let mut key = state
        .store()?
        .key(&key_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local key not found"))?;
    if key.enabled == enabled {
        return state.snapshot().await.map_err(Into::into);
    }
    if enabled {
        validate_key_record(&state, &key, true)?;
    }
    let (old_sources, old_keys) = current_records(&state)?;
    key.enabled = enabled;
    state.store()?.upsert_key(key)?;
    sync_records_or_rollback(&state, old_sources, old_keys).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn delete_local_gateway_key(
    key_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let key = state
        .store()?
        .key(&key_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local key not found"))?;
    let old_secret = secret_store::load(&key.secret_ref)?;
    let (old_sources, old_keys) = current_records(&state)?;
    let keys = old_keys
        .iter()
        .filter(|candidate| candidate.id != key_id)
        .cloned()
        .collect();
    state.store()?.replace_records(old_sources.clone(), keys)?;
    sync_records_or_rollback(&state, old_sources.clone(), old_keys.clone()).await?;

    if let Err(cleanup) = secret_store::delete(&key.secret_ref) {
        if let Some(secret) = old_secret {
            secret_store::save(&key.secret_ref, &secret).map_err(|restore| {
                LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("{cleanup}; failed to restore local key secret: {restore}"),
                )
            })?;
            let (deleted_sources, deleted_keys) = current_records(&state)?;
            let restore_records = { state.store()?.replace_records(old_sources, old_keys) };
            if let Err(restore) = restore_records {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("{cleanup}; failed to restore deleted local key: {restore}"),
                )
                .into());
            }
            if let Err(restore) =
                sync_records_or_rollback(&state, deleted_sources, deleted_keys).await
            {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("{cleanup}; failed to restore gateway after key cleanup: {restore}"),
                )
                .into());
            }
        }
        return Err(cleanup.into());
    }
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn rotate_local_gateway_key(
    key_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<GeneratedLocalKey> {
    let _mutation = state.setup_guard().await;
    let key = state
        .store()?
        .key(&key_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local key not found"))?;
    let old_secret = secret_store::load(&key.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local key secret is missing"))?;
    let secret = format!("zlr_{}", Uuid::new_v4().simple());
    secret_store::save(&key.secret_ref, &secret)?;
    restart_after_secret_change(&state, &key.secret_ref, &old_secret).await?;
    Ok(GeneratedLocalKey { key, secret })
}

#[tauri::command]
pub async fn update_local_routing(
    input: UpdateRoutingInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let old_gateway = state.store()?.gateway().clone();
    let mut gateway = old_gateway.clone();
    gateway.max_retry_candidates = input.max_retry_candidates;
    gateway.session_affinity = input.session_affinity;
    gateway.session_affinity_ttl_seconds = input.session_affinity_ttl_seconds;
    if gateway == old_gateway {
        return state.snapshot().await.map_err(Into::into);
    }
    state.store()?.replace_gateway(gateway)?;
    sync_gateway_or_rollback(&state, old_gateway).await?;
    state.snapshot().await.map_err(Into::into)
}

fn validate_key_record(
    state: &DesktopState,
    key: &LocalGatewayKeyRecord,
    require_usable_scope: bool,
) -> LocalResult<()> {
    if key.label.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "local key label must not be empty",
        ));
    }
    if let Some(source_ids) = &key.source_ids {
        let store = state.store()?;
        if source_ids.iter().any(|id| store.source(id).is_none()) {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "local key scope contains an unknown source",
            ));
        }
    }
    if let Some(account_ids) = &key.account_ids {
        let store = state.store()?;
        if account_ids.iter().any(|id| store.account(id).is_none()) {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "local key scope contains an unknown account",
            ));
        }
    }
    if secret_store::load(&key.secret_ref)?.is_none() && state.store()?.key(&key.id).is_some() {
        return Err(LocalPoolError::new(
            ErrorCode::NotFound,
            "local key secret is missing",
        ));
    }
    if require_usable_scope && !has_usable_source(state, key)? {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "local key must include at least one enabled, non-draining candidate",
        ));
    }
    Ok(())
}

pub(super) fn has_usable_source(
    state: &DesktopState,
    key: &LocalGatewayKeyRecord,
) -> LocalResult<bool> {
    let store = state.store()?;
    for source in store.sources() {
        let scoped = key
            .source_ids
            .as_ref()
            .is_none_or(|ids| ids.iter().any(|id| id == &source.id));
        if scoped
            && source.enabled
            && !source.draining
            && secret_store::load(&source.secret_ref)?.is_some()
        {
            return Ok(true);
        }
    }
    for account in store.accounts() {
        let scoped = key
            .account_ids
            .as_ref()
            .is_none_or(|ids| ids.iter().any(|id| id == &account.account.id));
        if !scoped || !account.account.enabled || account.account.draining {
            continue;
        }
        let mut secrets_available = !account.account.secret_refs.is_empty();
        for secret_ref in &account.account.secret_refs {
            if secret_store::load(secret_ref)?.is_none() {
                secrets_available = false;
                break;
            }
        }
        if secrets_available {
            return Ok(true);
        }
    }
    Ok(false)
}

fn current_records(
    state: &DesktopState,
) -> LocalResult<(
    Vec<crate::local_pool::models::ProviderSourceRecord>,
    Vec<LocalGatewayKeyRecord>,
)> {
    let store = state.store()?;
    Ok((store.sources().to_vec(), store.keys().to_vec()))
}

fn cleanup_created_secret(secret_ref: &str, cause: &LocalPoolError) -> LocalResult<()> {
    secret_store::delete(secret_ref).map_err(|cleanup| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "{}; secret cleanup failed: {}",
                cause.message, cleanup.message
            ),
        )
    })
}

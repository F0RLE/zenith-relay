use super::{
    clean_label, normalized_values, runtime_error, store_error, vault_error, ManagementError,
};
use crate::state::{generate_pool_key, now_ms, AppState, GatewayKeyRecord, SYSTEM_GATEWAY_KEY_ID};
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Serialize;
use std::collections::BTreeSet;
use std::sync::Arc;
use zenith_relay_core::protocol::{
    ClientAccessDocument, ClientKeyCreateInput, ClientKeyPatch, ClientWireApi, GeneratedClientKey,
    KeySummary, ProfileKeyRotation, CLIENT_ACCESS_SCHEMA_VERSION,
    PROFILE_KEY_ROTATION_SCHEMA_VERSION,
};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/keys", get(list_keys).post(create_key))
        .route("/keys/{id}", patch(update_key).delete(delete_key))
        .route("/keys/{id}/rotate", post(rotate_key))
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

const PROFILE_KEY_ROTATION_PREFIX: &str = "key_profile_rotation_";

const MAX_CLIENT_SOFT_BUDGET_MICRO_USD: u64 = 1_000_000_000_000;

pub async fn list_keys(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ClientAccessDocument>, ManagementError> {
    Ok(Json(ClientAccessDocument {
        schema_version: CLIENT_ACCESS_SCHEMA_VERSION,
        keys: state.snapshot().map_err(store_error)?.keys,
    }))
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
            ManagementError::internal("system_key_missing", "system client key is unavailable")
        })?;
    let secret = state
        .vault
        .load(&key.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::internal("system_key_missing", "system client key is unavailable")
        })?;
    if !key.enabled {
        let old = key.clone();
        key.enabled = true;
        state.store.save_key(&key).map_err(store_error)?;
        if let Err(error) = state.rebuild_runtime().await {
            let _ = state.store.save_key(&old);
            let _ = state.rebuild_runtime().await;
            return Err(runtime_error(error));
        }
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
            ManagementError::internal("system_key_missing", "system client key is unavailable")
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
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.store.delete_key(&rotation_id);
        let _ = state.vault.delete(&secret_ref);
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
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
            ManagementError::internal("system_key_missing", "system client key is unavailable")
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
                "profile key rotation was not found",
            )
        })?;
    let old_secret = state
        .vault
        .load(&current.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::internal("system_key_missing", "system client key is unavailable")
        })?;
    state
        .vault
        .save(&current.secret_ref, &new_secret)
        .map_err(vault_error)?;
    for (key, _) in &rotations {
        if let Err(error) = state.store.delete_key(&key.id) {
            restore_profile_rotation(&state, &current, &old_secret, &rotations);
            let _ = state.rebuild_runtime().await;
            return Err(store_error(error));
        }
        if let Err(error) = state.vault.delete(&key.secret_ref) {
            restore_profile_rotation(&state, &current, &old_secret, &rotations);
            let _ = state.rebuild_runtime().await;
            return Err(vault_error(error));
        }
    }
    if let Err(error) = state.rebuild_runtime().await {
        restore_profile_rotation(&state, &current, &old_secret, &rotations);
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
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
                "profile key rotation was not found",
            )
        })?;
    let secret = state.vault.load(&key.secret_ref).map_err(vault_error)?;
    state.store.delete_key(&id).map_err(store_error)?;
    if let Err(error) = state.vault.delete(&key.secret_ref) {
        let _ = state.store.save_key(&key);
        return Err(vault_error(error));
    }
    if let Err(error) = state.rebuild_runtime().await {
        if let Some(secret) = secret.as_deref() {
            let _ = state.vault.save(&key.secret_ref, secret);
        }
        let _ = state.store.save_key(&key);
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn create_key(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ClientKeyCreateInput>,
) -> Result<impl IntoResponse, ManagementError> {
    validate_client_access_schema(input.schema_version)?;
    let id = format!("key_{}", uuid::Uuid::new_v4().simple());
    let secret_ref = format!("key:{id}");
    let secret = generate_pool_key();
    let record = GatewayKeyRecord {
        id,
        label: clean_label(&input.label, "key label")?,
        enabled: true,
        system: false,
        secret_ref: secret_ref.clone(),
        source_ids: normalize_scope(input.source_ids),
        account_ids: normalize_scope(input.account_ids),
        allowed_models: normalized_values(input.allowed_models),
        excluded_models: normalized_values(input.excluded_models),
        model_prefix: normalize_prefix(input.model_prefix),
        wire_apis: normalize_wire_apis(input.wire_apis)?,
        soft_budget_micro_usd: validate_soft_budget(input.soft_budget_micro_usd)?,
        created_at_ms: now_ms(),
        last_used_at_ms: None,
    };
    state
        .vault
        .save(&secret_ref, &secret)
        .map_err(vault_error)?;
    if let Err(error) = state.store.save_key(&record) {
        let _ = state.vault.delete(&secret_ref);
        return Err(store_error(error));
    }
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.store.delete_key(&record.id);
        let _ = state.vault.delete(&secret_ref);
        return Err(runtime_error(error));
    }
    Ok((
        StatusCode::CREATED,
        [(header::CACHE_CONTROL, "no-store")],
        Json(GeneratedClientKey {
            schema_version: CLIENT_ACCESS_SCHEMA_VERSION,
            key: key_summary(&state, &record)?,
            secret,
        }),
    ))
}

pub async fn update_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<ClientKeyPatch>,
) -> Result<Json<KeySummary>, ManagementError> {
    validate_client_access_schema(input.schema_version)?;
    let mut record = find_key(&state, &id)?;
    reject_system_key_mutation(&record)?;
    let old = record.clone();
    if let Some(value) = input.label {
        record.label = clean_label(&value, "key label")?;
    }
    if let Some(value) = input.enabled {
        record.enabled = value;
    }
    if let Some(value) = input.source_ids {
        record.source_ids = normalize_scope(value);
    }
    if let Some(value) = input.account_ids {
        record.account_ids = normalize_scope(value);
    }
    if let Some(value) = input.allowed_models {
        record.allowed_models = normalized_values(value);
    }
    if let Some(value) = input.excluded_models {
        record.excluded_models = normalized_values(value);
    }
    if let Some(value) = input.model_prefix {
        record.model_prefix = normalize_prefix(value);
    }
    if let Some(value) = input.wire_apis {
        record.wire_apis = normalize_wire_apis(value)?;
    }
    if let Some(value) = input.soft_budget_micro_usd {
        record.soft_budget_micro_usd = validate_soft_budget(value)?;
    }
    state.store.save_key(&record).map_err(store_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.store.save_key(&old);
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(Json(key_summary(&state, &record)?))
}

pub async fn delete_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    let record = find_key(&state, &id)?;
    reject_system_key_mutation(&record)?;
    let secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .unwrap_or_default();
    state.store.delete_key(&id).map_err(store_error)?;
    state
        .vault
        .delete(&record.secret_ref)
        .map_err(vault_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.vault.save(&record.secret_ref, &secret);
        let _ = state.store.save_key(&record);
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn rotate_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ManagementError> {
    let record = find_key(&state, &id)?;
    reject_system_key_mutation(&record)?;
    let old_secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .unwrap_or_default();
    let secret = generate_pool_key();
    state
        .vault
        .save(&record.secret_ref, &secret)
        .map_err(vault_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.vault.save(&record.secret_ref, &old_secret);
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok((
        [(header::CACHE_CONTROL, "no-store")],
        Json(GeneratedClientKey {
            schema_version: CLIENT_ACCESS_SCHEMA_VERSION,
            key: key_summary(&state, &record)?,
            secret,
        }),
    ))
}

fn find_key(state: &AppState, id: &str) -> Result<GatewayKeyRecord, ManagementError> {
    state
        .store
        .keys()
        .map_err(store_error)?
        .into_iter()
        .find(|record| record.id == id)
        .ok_or_else(|| ManagementError::not_found("key_not_found", "gateway key not found"))
}

fn key_summary(state: &AppState, record: &GatewayKeyRecord) -> Result<KeySummary, ManagementError> {
    state
        .snapshot()
        .map_err(store_error)?
        .keys
        .into_iter()
        .find(|value| value.id == record.id)
        .ok_or_else(|| ManagementError::internal("snapshot_missing", "key snapshot missing"))
}

fn normalize_scope(values: Option<Vec<String>>) -> Option<Vec<String>> {
    values.map(normalized_values)
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
            "profile key rotation ID is invalid",
        ));
    }
    Ok(())
}

fn restore_profile_rotation(
    state: &AppState,
    current: &GatewayKeyRecord,
    current_secret: &str,
    rotations: &[(GatewayKeyRecord, Option<String>)],
) {
    let _ = state.vault.save(&current.secret_ref, current_secret);
    for (key, secret) in rotations {
        if let Some(secret) = secret.as_deref() {
            let _ = state.vault.save(&key.secret_ref, secret);
        }
        let _ = state.store.save_key(key);
    }
}

fn normalize_wire_apis(
    values: Option<Vec<ClientWireApi>>,
) -> Result<Option<Vec<ClientWireApi>>, ManagementError> {
    let values = values.map(|values| {
        values
            .into_iter()
            // Images is a legacy scope; image requests are governed by the
            // Chat Completions client surface, so keep one canonical value.
            .map(|value| match value {
                ClientWireApi::Images => ClientWireApi::ChatCompletions,
                value => value,
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    });
    if values.as_ref().is_some_and(Vec::is_empty) {
        return Err(ManagementError::validation(
            "client_wire_scope_empty",
            "at least one client API must be allowed",
        ));
    }
    Ok(values)
}

fn validate_soft_budget(value: Option<u64>) -> Result<Option<u64>, ManagementError> {
    if value.is_some_and(|value| value == 0 || value > MAX_CLIENT_SOFT_BUDGET_MICRO_USD) {
        return Err(ManagementError::validation(
            "client_soft_budget_invalid",
            "client soft budget must be between 1 micro-USD and 1,000,000 USD",
        ));
    }
    Ok(value)
}

fn validate_client_access_schema(schema_version: u16) -> Result<(), ManagementError> {
    if schema_version != CLIENT_ACCESS_SCHEMA_VERSION {
        return Err(ManagementError::validation(
            "client_access_schema_unsupported",
            format!("client access schema version {schema_version} is not supported"),
        ));
    }
    Ok(())
}

fn normalize_prefix(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty())
}

fn reject_system_key_mutation(record: &GatewayKeyRecord) -> Result<(), ManagementError> {
    if record.system {
        return Err(ManagementError::new(
            StatusCode::CONFLICT,
            "system_key_managed",
            "system client key is managed automatically",
            "keys",
            false,
        ));
    }
    Ok(())
}

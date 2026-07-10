use crate::{
    state::{
        identity_hint, now_ms, AccountCredential, AppState, GatewayKeyRecord, ServerAccountRecord,
        SourceRecord,
    },
    store::PendingImport,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};
use url::{Host, Url};
use zenith_relay_core::{
    accounts::{AccountAuthState, AccountHealthState},
    automations::{WakeHistory, WakeTask},
    protocol::{
        AccountSummary, ApiError, ErrorEnvelope, HealthResponse, KeySummary, RuntimeStateSnapshot,
        SourceSummary, UsagePage,
    },
    quota::QuotaSnapshot,
    ProviderSource, WireApi,
};

const MAX_SECRET_BYTES: usize = 64 * 1024;
const DEFAULT_CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

pub async fn health(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        server_id: state.capabilities.server_id.clone(),
        started_at_ms: state.started_at_ms,
    })
}

pub async fn capabilities(
    State(state): State<Arc<AppState>>,
) -> Json<zenith_relay_core::protocol::Capabilities> {
    Json(state.capabilities.clone())
}

pub async fn state_snapshot(
    State(state): State<Arc<AppState>>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    Ok(Json(state.snapshot().map_err(store_error)?))
}

pub async fn list_sources(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<SourceSummary>>, ManagementError> {
    Ok(Json(state.snapshot().map_err(store_error)?.sources))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInput {
    name: String,
    base_url: String,
    api_key: String,
    wire_api: WireApi,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    allowed_models: Vec<String>,
    #[serde(default)]
    excluded_models: Vec<String>,
    #[serde(default)]
    priority: i32,
    #[serde(default = "default_weight")]
    weight: u32,
}

pub async fn create_source(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SourceInput>,
) -> Result<(StatusCode, Json<SourceSummary>), ManagementError> {
    validate_secret(&input.api_key, "source API key")?;
    let api_key = input.api_key.clone();
    let id = format!("source_{}", uuid::Uuid::new_v4().simple());
    let secret_ref = format!("source:{id}");
    let record = source_record(id, secret_ref.clone(), input)?;
    state
        .vault
        .save(&secret_ref, &api_key)
        .map_err(vault_error)?;
    if let Err(error) = state.store.save_source(&record) {
        let _ = state.vault.delete(&secret_ref);
        return Err(store_error(error));
    }
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.store.delete_source(&record.id);
        let _ = state.vault.delete(&secret_ref);
        return Err(runtime_error(error));
    }
    Ok((StatusCode::CREATED, Json(source_summary(&state, &record)?)))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePatch {
    name: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    wire_api: Option<WireApi>,
    models: Option<Vec<String>>,
    allowed_models: Option<Vec<String>>,
    excluded_models: Option<Vec<String>>,
    enabled: Option<bool>,
    draining: Option<bool>,
    priority: Option<i32>,
    weight: Option<u32>,
}

pub async fn update_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<SourcePatch>,
) -> Result<Json<SourceSummary>, ManagementError> {
    let mut record = find_source(&state, &id)?;
    let old_record = record.clone();
    let old_secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found("source_secret_missing", "source secret missing")
        })?;
    if let Some(value) = input.name {
        record.name = clean_label(&value, "source name")?;
    }
    if let Some(value) = input.base_url {
        record.base_url = value.trim().to_string();
    }
    if let Some(value) = input.wire_api {
        record.wire_api = value;
    }
    if let Some(value) = input.models {
        record.models = normalized_values(value);
    }
    if let Some(value) = input.allowed_models {
        record.allowed_models = normalized_values(value);
    }
    if let Some(value) = input.excluded_models {
        record.excluded_models = normalized_values(value);
    }
    if let Some(value) = input.enabled {
        record.enabled = value;
    }
    if let Some(value) = input.draining {
        record.draining = value;
    }
    if let Some(value) = input.priority {
        record.priority = value;
    }
    if let Some(value) = input.weight {
        record.weight = valid_weight(value)?;
    }
    validate_source_record(&record, input.api_key.as_deref().unwrap_or(&old_secret))?;
    if let Some(secret) = input.api_key.as_deref() {
        validate_secret(secret, "source API key")?;
        state
            .vault
            .save(&record.secret_ref, secret)
            .map_err(vault_error)?;
    }
    if let Err(error) = state.store.save_source(&record) {
        let _ = state.vault.save(&record.secret_ref, &old_secret);
        return Err(store_error(error));
    }
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.store.save_source(&old_record);
        let _ = state.vault.save(&old_record.secret_ref, &old_secret);
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(Json(source_summary(&state, &record)?))
}

pub async fn delete_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    let record = find_source(&state, &id)?;
    let secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found("source_secret_missing", "source secret missing")
        })?;
    let old_keys = state.store.keys().map_err(store_error)?;
    let mut scoped_keys = old_keys.clone();
    for key in &mut scoped_keys {
        if let Some(ids) = &mut key.source_ids {
            ids.retain(|value| value != &id);
        }
        state.store.save_key(key).map_err(store_error)?;
    }
    state.store.delete_source(&id).map_err(store_error)?;
    state
        .vault
        .delete(&record.secret_ref)
        .map_err(vault_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.vault.save(&record.secret_ref, &secret);
        let _ = state.store.save_source(&record);
        for key in old_keys {
            let _ = state.store.save_key(&key);
        }
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_accounts(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<AccountSummary>>, ManagementError> {
    Ok(Json(state.snapshot().map_err(store_error)?.accounts))
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountImportInput {
    label: String,
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_at_ms: Option<u64>,
    chatgpt_account_id: String,
    responses_url: Option<String>,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    allowed_models: Vec<String>,
    #[serde(default)]
    excluded_models: Vec<String>,
    #[serde(default)]
    priority: i32,
    #[serde(default = "default_weight")]
    weight: u32,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountImportPreview {
    session_id: String,
    account_id: String,
    duplicate_account_id: Option<String>,
    label: String,
    identity_hint: String,
    models: Vec<String>,
    auth_state: AccountAuthState,
    expires_at_ms: Option<u64>,
    allowed_models: Vec<String>,
    excluded_models: Vec<String>,
    priority: i32,
    weight: u32,
}

pub async fn preview_account_import(
    State(state): State<Arc<AppState>>,
    Json(input): Json<AccountImportInput>,
) -> Result<(StatusCode, Json<AccountImportPreview>), ManagementError> {
    validate_secret(&input.access_token, "access token")?;
    if let Some(value) = input.refresh_token.as_deref() {
        validate_secret(value, "refresh token")?;
    }
    if let Some(value) = input.id_token.as_deref() {
        validate_secret(value, "ID token")?;
    }
    let label = clean_label(&input.label, "account label")?;
    let chatgpt_account_id = clean_identifier(&input.chatgpt_account_id, "account id")?;
    let responses_url = validate_account_responses_url(input.responses_url.as_deref())?;
    let identity_hint = identity_hint(&chatgpt_account_id);
    let duplicate_account_id = state
        .store
        .accounts()
        .map_err(store_error)?
        .into_iter()
        .find(|record| record.identity_hint == identity_hint)
        .map(|record| record.id);
    let account_id = duplicate_account_id
        .clone()
        .unwrap_or_else(|| format!("account_{}", uuid::Uuid::new_v4().simple()));
    let session_id = format!("import_{}", uuid::Uuid::new_v4().simple());
    let secret_ref = format!("account:{account_id}:{}", uuid::Uuid::new_v4().simple());
    let credential = AccountCredential {
        access_token: input.access_token,
        refresh_token: nonempty(input.refresh_token),
        id_token: nonempty(input.id_token),
        expires_at_ms: input.expires_at_ms,
        issued_at_ms: now_ms(),
        generation: 0,
        chatgpt_account_id,
        responses_url,
    };
    credential.tokens().map_err(validation_error)?;
    let auth_state = if credential.refresh_token.is_some() {
        AccountAuthState::Active
    } else {
        AccountAuthState::DegradedAccessOnly
    };
    let preview = AccountImportPreview {
        session_id: session_id.clone(),
        account_id,
        duplicate_account_id,
        label,
        identity_hint,
        models: normalized_values(input.models),
        auth_state,
        expires_at_ms: credential.expires_at_ms,
        allowed_models: normalized_values(input.allowed_models),
        excluded_models: normalized_values(input.excluded_models),
        priority: input.priority,
        weight: valid_weight(input.weight)?,
    };
    state
        .vault
        .save(
            &secret_ref,
            &serde_json::to_string(&credential).map_err(|_| {
                ManagementError::internal("import_serialize", "import could not be prepared")
            })?,
        )
        .map_err(vault_error)?;
    let pending = PendingImport {
        id: session_id,
        preview_json: serde_json::to_string(&preview).map_err(|_| {
            ManagementError::internal("preview_serialize", "preview could not be saved")
        })?,
        secret_ref,
        created_at_ms: now_ms(),
    };
    if let Err(error) = state.store.save_pending_import(&pending) {
        let _ = state.vault.delete(&pending.secret_ref);
        return Err(store_error(error));
    }
    Ok((StatusCode::CREATED, Json(preview)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmImportInput {
    session_id: String,
}

pub async fn confirm_account_import(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ConfirmImportInput>,
) -> Result<Json<AccountSummary>, ManagementError> {
    let pending = state
        .store
        .pending_import(&input.session_id)
        .map_err(store_error)?
        .ok_or_else(|| {
            ManagementError::not_found("import_not_found", "import session not found")
        })?;
    if now_ms().saturating_sub(pending.created_at_ms) > 30 * 60 * 1_000 {
        let _ = state.store.delete_pending_import(&input.session_id);
        let _ = state.vault.delete(&pending.secret_ref);
        return Err(ManagementError::validation(
            "import_expired",
            "import session expired",
        ));
    }
    let preview: AccountImportPreview =
        serde_json::from_str(&pending.preview_json).map_err(|_| {
            ManagementError::internal("preview_invalid", "stored import preview is invalid")
        })?;
    let existing = state
        .store
        .accounts()
        .map_err(store_error)?
        .into_iter()
        .find(|record| record.id == preview.account_id);
    let record = ServerAccountRecord {
        id: preview.account_id.clone(),
        label: preview.label,
        identity_hint: preview.identity_hint,
        enabled: true,
        draining: false,
        source_id: "openai_codex".to_string(),
        secret_ref: pending.secret_ref.clone(),
        auth_state: preview.auth_state,
        health: AccountHealthState::Healthy,
        models: preview.models,
        allowed_models: preview.allowed_models,
        excluded_models: preview.excluded_models,
        priority: preview.priority,
        weight: preview.weight,
        subscription: existing
            .as_ref()
            .map(|value| value.subscription.clone())
            .unwrap_or_default(),
        quota: existing
            .as_ref()
            .map(|value| value.quota.clone())
            .unwrap_or_default(),
        cooldowns: BTreeMap::new(),
        consecutive_failures: 0,
        last_used_at_ms: existing.as_ref().and_then(|value| value.last_used_at_ms),
        last_error_code: None,
    };
    state.store.save_account(&record).map_err(store_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        match existing.as_ref() {
            Some(previous) => {
                let _ = state.store.save_account(previous);
            }
            None => {
                let _ = state.store.delete_account(&record.id);
            }
        }
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    state
        .store
        .delete_pending_import(&input.session_id)
        .map_err(store_error)?;
    if let Some(previous) = existing {
        if previous.secret_ref != record.secret_ref {
            let _ = state.vault.delete(&previous.secret_ref);
        }
    }
    Ok(Json(account_summary(&state, &record)?))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPatch {
    label: Option<String>,
    enabled: Option<bool>,
    draining: Option<bool>,
    allowed_models: Option<Vec<String>>,
    excluded_models: Option<Vec<String>>,
    priority: Option<i32>,
    weight: Option<u32>,
}

pub async fn update_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<AccountPatch>,
) -> Result<Json<AccountSummary>, ManagementError> {
    let mut record = find_account(&state, &id)?;
    let old = record.clone();
    if let Some(value) = input.label {
        record.label = clean_label(&value, "account label")?;
    }
    if let Some(value) = input.enabled {
        record.enabled = value;
    }
    if let Some(value) = input.draining {
        record.draining = value;
    }
    if let Some(value) = input.allowed_models {
        record.allowed_models = normalized_values(value);
    }
    if let Some(value) = input.excluded_models {
        record.excluded_models = normalized_values(value);
    }
    if let Some(value) = input.priority {
        record.priority = value;
    }
    if let Some(value) = input.weight {
        record.weight = valid_weight(value)?;
    }
    state.store.save_account(&record).map_err(store_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.store.save_account(&old);
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(Json(account_summary(&state, &record)?))
}

pub async fn delete_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    let record = find_account(&state, &id)?;
    let secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found("account_secret_missing", "account secret missing")
        })?;
    let old_keys = state.store.keys().map_err(store_error)?;
    for mut key in old_keys.clone() {
        if let Some(ids) = &mut key.account_ids {
            ids.retain(|value| value != &id);
        }
        state.store.save_key(&key).map_err(store_error)?;
    }
    state.store.delete_account(&id).map_err(store_error)?;
    state
        .vault
        .delete(&record.secret_ref)
        .map_err(vault_error)?;
    state.token_authority.remove(&id);
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.vault.save(&record.secret_ref, &secret);
        let _ = state.store.save_account(&record);
        for key in old_keys {
            let _ = state.store.save_key(&key);
        }
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn list_keys(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<KeySummary>>, ManagementError> {
    Ok(Json(state.snapshot().map_err(store_error)?.keys))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInput {
    label: String,
    source_ids: Option<Vec<String>>,
    account_ids: Option<Vec<String>>,
    #[serde(default)]
    allowed_models: Vec<String>,
    #[serde(default)]
    excluded_models: Vec<String>,
    model_prefix: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedKey {
    key: KeySummary,
    secret: String,
}

pub async fn create_key(
    State(state): State<Arc<AppState>>,
    Json(input): Json<KeyInput>,
) -> Result<(StatusCode, Json<GeneratedKey>), ManagementError> {
    let id = format!("key_{}", uuid::Uuid::new_v4().simple());
    let secret_ref = format!("key:{id}");
    let secret = generate_pool_key();
    let record = GatewayKeyRecord {
        id,
        label: clean_label(&input.label, "key label")?,
        enabled: true,
        secret_ref: secret_ref.clone(),
        source_ids: normalize_scope(input.source_ids),
        account_ids: normalize_scope(input.account_ids),
        allowed_models: normalized_values(input.allowed_models),
        excluded_models: normalized_values(input.excluded_models),
        model_prefix: normalize_prefix(input.model_prefix),
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
        Json(GeneratedKey {
            key: key_summary(&state, &record)?,
            secret,
        }),
    ))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyPatch {
    label: Option<String>,
    enabled: Option<bool>,
    source_ids: Option<Option<Vec<String>>>,
    account_ids: Option<Option<Vec<String>>>,
    allowed_models: Option<Vec<String>>,
    excluded_models: Option<Vec<String>>,
    model_prefix: Option<Option<String>>,
}

pub async fn update_key(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<KeyPatch>,
) -> Result<Json<KeySummary>, ManagementError> {
    let mut record = find_key(&state, &id)?;
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
) -> Result<Json<GeneratedKey>, ManagementError> {
    let record = find_key(&state, &id)?;
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
    Ok(Json(GeneratedKey {
        key: key_summary(&state, &record)?,
        secret,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaItem {
    account_id: String,
    quota: QuotaSnapshot,
}

pub async fn quota(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<QuotaItem>>, ManagementError> {
    Ok(Json(
        state
            .store
            .accounts()
            .map_err(store_error)?
            .into_iter()
            .map(|record| QuotaItem {
                account_id: record.id,
                quota: record.quota,
            })
            .collect(),
    ))
}

#[derive(Serialize)]
pub struct ModelList {
    data: Vec<ModelItem>,
}
#[derive(Serialize)]
struct ModelItem {
    id: String,
    object: &'static str,
    owned_by: &'static str,
}

pub async fn models(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ModelList>, ManagementError> {
    let models = state
        .snapshot()
        .map_err(store_error)?
        .gateway
        .visible_model_ids
        .into_iter()
        .map(|id| ModelItem {
            id,
            object: "model",
            owned_by: "user",
        })
        .collect();
    Ok(Json(ModelList { data: models }))
}

#[derive(Deserialize)]
pub struct UsageQuery {
    page: Option<u32>,
    page_size: Option<u32>,
}

pub async fn usage(
    State(state): State<Arc<AppState>>,
    Query(query): Query<UsageQuery>,
) -> Result<Json<UsagePage>, ManagementError> {
    Ok(Json(
        state
            .store
            .usage_page(query.page.unwrap_or(1), query.page_size.unwrap_or(50))
            .map_err(store_error)?,
    ))
}

pub async fn start_gateway(
    State(state): State<Arc<AppState>>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    state.store.set_gateway_enabled(true).map_err(store_error)?;
    state.rebuild_runtime().await.map_err(runtime_error)?;
    Ok(Json(state.snapshot().map_err(store_error)?))
}

pub async fn stop_gateway(
    State(state): State<Arc<AppState>>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    state
        .store
        .set_gateway_enabled(false)
        .map_err(store_error)?;
    Ok(Json(state.snapshot().map_err(store_error)?))
}

pub async fn list_wake_tasks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<WakeTask>>, ManagementError> {
    Ok(Json(state.store.wake_tasks().map_err(store_error)?))
}

pub async fn create_wake_task(
    State(state): State<Arc<AppState>>,
    Json(mut task): Json<WakeTask>,
) -> Result<(StatusCode, Json<WakeTask>), ManagementError> {
    if task.id.trim().is_empty() {
        task.id = format!("wake_{}", uuid::Uuid::new_v4().simple());
    }
    let timestamp = now_ms();
    task.created_at_ms = timestamp;
    task.updated_at_ms = timestamp;
    task.validate()
        .map_err(|error| validation_error(format!("{error:?}")))?;
    state.store.save_wake_task(&task).map_err(store_error)?;
    Ok((StatusCode::CREATED, Json(task)))
}

pub async fn update_wake_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut task): Json<WakeTask>,
) -> Result<Json<WakeTask>, ManagementError> {
    if !state
        .store
        .wake_tasks()
        .map_err(store_error)?
        .iter()
        .any(|value| value.id == id)
    {
        return Err(ManagementError::not_found(
            "wake_task_not_found",
            "wake task not found",
        ));
    }
    task.id = id;
    task.updated_at_ms = now_ms();
    task.validate()
        .map_err(|error| validation_error(format!("{error:?}")))?;
    state.store.save_wake_task(&task).map_err(store_error)?;
    Ok(Json(task))
}

pub async fn delete_wake_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    if !state.store.delete_wake_task(&id).map_err(store_error)? {
        return Err(ManagementError::not_found(
            "wake_task_not_found",
            "wake task not found",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeTestResult {
    task_id: String,
    status: &'static str,
}

pub async fn test_wake_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<WakeTestResult>, ManagementError> {
    if !state
        .store
        .wake_tasks()
        .map_err(store_error)?
        .iter()
        .any(|value| value.id == id)
    {
        return Err(ManagementError::not_found(
            "wake_task_not_found",
            "wake task not found",
        ));
    }
    Ok(Json(WakeTestResult {
        task_id: id,
        status: "queued_for_background_worker",
    }))
}

pub async fn wake_history(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<WakeHistory>>, ManagementError> {
    Ok(Json(
        state
            .store
            .wake_state()
            .map_err(store_error)?
            .history()
            .iter()
            .cloned()
            .collect(),
    ))
}

fn source_record(
    id: String,
    secret_ref: String,
    input: SourceInput,
) -> Result<SourceRecord, ManagementError> {
    let record = SourceRecord {
        id,
        name: clean_label(&input.name, "source name")?,
        enabled: true,
        draining: false,
        base_url: input.base_url.trim().to_string(),
        secret_ref,
        wire_api: input.wire_api,
        models: normalized_values(input.models),
        allowed_models: normalized_values(input.allowed_models),
        excluded_models: normalized_values(input.excluded_models),
        priority: input.priority,
        weight: valid_weight(input.weight)?,
        last_error_code: None,
    };
    validate_source_record(&record, &input.api_key)?;
    Ok(record)
}

fn validate_source_record(record: &SourceRecord, api_key: &str) -> Result<(), ManagementError> {
    ProviderSource {
        id: record.id.clone(),
        name: record.name.clone(),
        base_url: record.base_url.clone(),
        api_key: api_key.to_string(),
        wire_api: record.wire_api,
        models: record.models.clone(),
    }
    .validate()
    .map_err(|error| validation_error(error.to_string()))
}

fn find_source(state: &AppState, id: &str) -> Result<SourceRecord, ManagementError> {
    state
        .store
        .sources()
        .map_err(store_error)?
        .into_iter()
        .find(|record| record.id == id)
        .ok_or_else(|| ManagementError::not_found("source_not_found", "source not found"))
}

fn find_account(state: &AppState, id: &str) -> Result<ServerAccountRecord, ManagementError> {
    state
        .store
        .accounts()
        .map_err(store_error)?
        .into_iter()
        .find(|record| record.id == id)
        .ok_or_else(|| ManagementError::not_found("account_not_found", "account not found"))
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

fn source_summary(
    state: &AppState,
    record: &SourceRecord,
) -> Result<SourceSummary, ManagementError> {
    state
        .snapshot()
        .map_err(store_error)?
        .sources
        .into_iter()
        .find(|value| value.id == record.id)
        .ok_or_else(|| ManagementError::internal("snapshot_missing", "source snapshot missing"))
}

fn account_summary(
    state: &AppState,
    record: &ServerAccountRecord,
) -> Result<AccountSummary, ManagementError> {
    state
        .snapshot()
        .map_err(store_error)?
        .accounts
        .into_iter()
        .find(|value| value.id == record.id)
        .ok_or_else(|| ManagementError::internal("snapshot_missing", "account snapshot missing"))
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

fn validate_secret(value: &str, name: &str) -> Result<(), ManagementError> {
    if value.is_empty()
        || value.len() > MAX_SECRET_BYTES
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(validation_error(format!("{name} is invalid")))
    } else {
        Ok(())
    }
}

fn validate_account_responses_url(value: Option<&str>) -> Result<String, ManagementError> {
    let value = value.unwrap_or(DEFAULT_CODEX_RESPONSES_URL).trim();
    let url =
        Url::parse(value).map_err(|_| validation_error("account responses URL is invalid"))?;
    let loopback = match url.host() {
        Some(Host::Ipv4(value)) => value.is_loopback(),
        Some(Host::Ipv6(value)) => value.is_loopback(),
        Some(Host::Domain(value)) => value.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    let allowed = (url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case("chatgpt.com")))
        || (url.scheme() == "http" && loopback);
    if !allowed
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(validation_error("account responses URL is not allowed"));
    }
    Ok(url.to_string())
}

fn clean_label(value: &str, name: &str) -> Result<String, ManagementError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        Err(validation_error(format!("{name} is invalid")))
    } else {
        Ok(value.to_string())
    }
}

fn clean_identifier(value: &str, name: &str) -> Result<String, ManagementError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        Err(validation_error(format!("{name} is invalid")))
    } else {
        Ok(value.to_string())
    }
}

fn normalized_values(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty() && seen.insert(value.to_ascii_lowercase()))
        .collect()
}

fn normalize_scope(values: Option<Vec<String>>) -> Option<Vec<String>> {
    values.map(normalized_values)
}
fn normalize_prefix(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().trim_matches('/').to_string())
        .filter(|value| !value.is_empty())
}
fn nonempty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
fn default_weight() -> u32 {
    1
}
fn valid_weight(value: u32) -> Result<u32, ManagementError> {
    (value > 0)
        .then_some(value)
        .ok_or_else(|| validation_error("weight must be positive"))
}

fn generate_pool_key() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("zrs_{}", URL_SAFE_NO_PAD.encode(bytes))
}

#[derive(Debug)]
pub struct ManagementError {
    status: StatusCode,
    code: String,
    message: String,
    stage: String,
    retryable: bool,
}

impl ManagementError {
    fn validation(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, code, message, "validation", false)
    }
    fn not_found(code: &str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, code, message, "lookup", false)
    }
    fn internal(code: &str, message: impl Into<String>) -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            code,
            message,
            "server",
            true,
        )
    }
    fn new(
        status: StatusCode,
        code: &str,
        message: impl Into<String>,
        stage: &str,
        retryable: bool,
    ) -> Self {
        Self {
            status,
            code: code.to_string(),
            message: message.into(),
            stage: stage.to_string(),
            retryable,
        }
    }
}

impl IntoResponse for ManagementError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorEnvelope {
                error: ApiError {
                    code: self.code,
                    message: self.message,
                    stage: self.stage,
                    retryable: self.retryable,
                    request_id: uuid::Uuid::new_v4().to_string(),
                },
            }),
        )
            .into_response()
    }
}

fn validation_error(message: impl Into<String>) -> ManagementError {
    ManagementError::validation("invalid_request", message)
}
fn store_error(_error: String) -> ManagementError {
    ManagementError::internal("store_failed", "server storage operation failed")
}
fn vault_error(_error: String) -> ManagementError {
    ManagementError::internal("vault_failed", "encrypted secret storage operation failed")
}
fn runtime_error(_error: String) -> ManagementError {
    ManagementError::internal(
        "runtime_reload_failed",
        "runtime could not reload the new state",
    )
}

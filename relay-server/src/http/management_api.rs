use crate::{
    jobs,
    state::{
        generate_pool_key, identity_hint, now_ms, AccountCredential, AppState, GatewayKeyRecord,
        ServerAccountRecord, SourceRecord, COMMON_PROXY_SECRET_REF, MAX_SERVER_ACCOUNTS,
    },
    store::{PendingImport, MAX_MODEL_PRICE_MICRO_USD_PER_MILLION},
};
use axum::{
    body::{to_bytes, Body},
    extract::{Path, Query, State},
    http::{header, Request, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::Arc,
    time::Instant,
};
use tower::ServiceExt;
use url::{Host, Url};
use zenith_relay_core::{
    accounts::{
        build_account_export, normalize_account_export_description, AccountAuthState,
        AccountExportCredential, AccountExportDocument, AccountExportRequest, AccountHealthState,
    },
    automations::{AccountSelector, WakeHistory, WakeModelPolicy, WakeTask},
    discover_source_models, normalize_proxy_url,
    protocol::{
        AccountSummary, ApiError, ErrorEnvelope, GatewayDiagnostic, HealthResponse, KeySummary,
        RevealedAccountIdentity, RuntimeStateSnapshot, SourceSummary, UsagePage, UsageQuery,
        UsageRange,
    },
    quota::{parse_subscription_timestamp_ms, QuotaSnapshot, Subscription, SubscriptionInput},
    source_points_to_gateway, ApiModelPriceOverride, CandidateRuntimeSnapshot, DefaultServiceTier,
    ProviderSource, RoutingStrategy, WireApi,
};

const MAX_SECRET_BYTES: usize = 64 * 1024;
const MAX_IMPORT_BYTES: usize = 4 * 1024 * 1024;
const MAX_IMPORT_ITEMS: usize = 1_024;
const MAX_IMPORT_DEPTH: usize = 32;
const MAX_JWT_BYTES: usize = 64 * 1024;
const MAX_JWT_PAYLOAD_BYTES: usize = 16 * 1024;
const MAX_SYNCHRONOUS_IMPORT_PROBES: usize = 5;
const DEFAULT_CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const IMPORT_ERROR_MARKER: &str = "__zenith_import_error";

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

pub async fn runtime_order(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<CandidateRuntimeSnapshot>>, ManagementError> {
    Ok(Json(state.runtime_order().map_err(runtime_error)?))
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
    let mut record = source_record(id, secret_ref.clone(), input)?;
    ensure_not_server_self_source(&state, &record.base_url)?;
    record.models = discover_models(&record, &api_key).await?;
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
    in_pool: Option<bool>,
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
    if let Some(value) = input.in_pool {
        record.in_pool = value;
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
    ensure_not_server_self_source(&state, &record.base_url)?;
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

pub async fn test_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SourceSummary>, ManagementError> {
    let mut record = find_source(&state, &id)?;
    let previous = record.clone();
    let api_key = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found("source_secret_missing", "source secret missing")
        })?;
    ensure_not_server_self_source(&state, &record.base_url)?;
    record.models = match discover_models(&record, &api_key).await {
        Ok(models) => models,
        Err(error) => {
            record.last_error_code = Some(error.code.clone());
            state.store.save_source(&record).map_err(store_error)?;
            return Err(error);
        }
    };
    record.last_error_code = None;
    state.store.save_source(&record).map_err(store_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.store.save_source(&previous);
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(Json(source_summary(&state, &record)?))
}

pub async fn list_accounts(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<AccountSummary>>, ManagementError> {
    Ok(Json(state.snapshot().map_err(store_error)?.accounts))
}

pub async fn reveal_account_identity(
    Path(account_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Response, ManagementError> {
    let record = find_account(&state, &account_id)?;
    let secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::internal(
                "account_secret_missing",
                "stored account credential is unavailable",
            )
        })?;
    let credential: AccountCredential = serde_json::from_str(&secret).map_err(|_| {
        ManagementError::internal(
            "account_secret_invalid",
            "stored account credential is invalid",
        )
    })?;
    Ok(no_store_json(RevealedAccountIdentity {
        account_id,
        identity: credential.chatgpt_account_id,
    }))
}

pub async fn export_accounts(
    State(state): State<Arc<AppState>>,
    Json(input): Json<AccountExportRequest>,
) -> Result<Response, ManagementError> {
    input
        .validate()
        .map_err(|error| validation_error(error.to_string()))?;
    let mut accounts = Vec::with_capacity(input.account_ids.len());
    for account_id in &input.account_ids {
        let record = find_account(&state, account_id)?;
        let secret = state
            .vault
            .load(&record.secret_ref)
            .map_err(vault_error)?
            .ok_or_else(|| {
                ManagementError::internal(
                    "account_secret_missing",
                    "stored account credential is unavailable",
                )
            })?;
        let credential: AccountCredential = serde_json::from_str(&secret).map_err(|_| {
            ManagementError::internal(
                "account_secret_invalid",
                "stored account credential is invalid",
            )
        })?;
        accounts.push(AccountExportCredential {
            label: record.label,
            email: None,
            access_token: credential.access_token,
            refresh_token: credential.refresh_token,
            id_token: credential.id_token,
            account_id: Some(credential.chatgpt_account_id),
            user_id: None,
            organization_id: None,
            plan_type: record.subscription.plan_type.clone(),
            expires_at_ms: credential.expires_at_ms,
            issued_at_ms: credential.issued_at_ms,
            subscription_active_until_ms: record.subscription.active_until_ms,
            created_at_ms: credential.issued_at_ms,
            priority: record.priority,
            enabled: record.enabled,
        });
    }
    let document: AccountExportDocument = build_account_export(
        input.format,
        &accounts,
        now_ms(),
        input.description.as_deref(),
    )
    .map_err(|_| {
        ManagementError::internal(
            "account_export_failed",
            "account export could not be created",
        )
    })?;
    Ok(no_store_json(document))
}

fn no_store_json<T: Serialize>(value: T) -> Response {
    let mut response = Json(value).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store, max-age=0"),
    );
    response
        .headers_mut()
        .insert(header::PRAGMA, header::HeaderValue::from_static("no-cache"));
    response
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetProxyInput {
    proxy_url: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyPolicyInput {
    required: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignProxiesInput {
    account_ids: Vec<String>,
    proxy_urls: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyAssignmentResult {
    assigned: usize,
    unused: usize,
}

pub async fn set_common_proxy(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SetProxyInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let next = normalize_optional_proxy(input.proxy_url)?;
    let previous_configured = state.store.common_proxy_configured().map_err(store_error)?;
    let previous = state
        .vault
        .load(COMMON_PROXY_SECRET_REF)
        .map_err(vault_error)?;
    if previous_configured == next.is_some() && previous.as_deref() == next.as_deref() {
        return Ok(Json(state.snapshot().map_err(store_error)?));
    }
    save_optional_proxy(&state, COMMON_PROXY_SECRET_REF, next.as_deref())?;
    if let Err(error) = state.store.set_common_proxy_configured(next.is_some()) {
        restore_common_proxy(&state, previous_configured, previous.as_deref())?;
        return Err(store_error(error));
    }
    if let Err(error) = state.rebuild_runtime().await {
        restore_common_proxy(&state, previous_configured, previous.as_deref())?;
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(Json(state.snapshot().map_err(store_error)?))
}

pub async fn set_account_proxy_required(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ProxyPolicyInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let previous = state.store.account_proxy_required().map_err(store_error)?;
    if previous == input.required {
        return Ok(Json(state.snapshot().map_err(store_error)?));
    }
    state
        .store
        .set_account_proxy_required(input.required)
        .map_err(store_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        state
            .store
            .set_account_proxy_required(previous)
            .map_err(store_error)?;
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(Json(state.snapshot().map_err(store_error)?))
}

pub async fn set_account_proxy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<SetProxyInput>,
) -> Result<Json<AccountSummary>, ManagementError> {
    let record = find_account(&state, &id)?;
    let previous = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found("account_secret_missing", "account secret missing")
        })?;
    let mut credential: AccountCredential = serde_json::from_str(&previous).map_err(|_| {
        ManagementError::internal("account_secret_invalid", "account secret is invalid")
    })?;
    let next = normalize_optional_proxy(input.proxy_url)?;
    if credential.proxy_url == next {
        return Ok(Json(account_summary(&state, &record)?));
    }
    credential.proxy_url = next;
    let encoded = serde_json::to_string(&credential).map_err(|_| {
        ManagementError::internal(
            "account_secret_serialize",
            "account secret could not be saved",
        )
    })?;
    state
        .vault
        .save(&record.secret_ref, &encoded)
        .map_err(vault_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        state
            .vault
            .save(&record.secret_ref, &previous)
            .map_err(vault_error)?;
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(Json(account_summary(&state, &record)?))
}

pub async fn assign_account_proxies(
    State(state): State<Arc<AppState>>,
    Json(input): Json<AssignProxiesInput>,
) -> Result<Json<ProxyAssignmentResult>, ManagementError> {
    if input.account_ids.is_empty()
        || input.account_ids.len() > MAX_SERVER_ACCOUNTS
        || input.proxy_urls.len() < input.account_ids.len()
    {
        return Err(ManagementError::validation(
            "proxy_assignment_invalid",
            "proxy list must contain one URL per selected account",
        ));
    }
    let mut seen = HashSet::new();
    if input
        .account_ids
        .iter()
        .any(|account_id| !seen.insert(account_id.clone()))
    {
        return Err(ManagementError::validation(
            "proxy_assignment_duplicate",
            "proxy assignment contains duplicate account ids",
        ));
    }
    let mut updates = Vec::with_capacity(input.account_ids.len());
    for (account_id, proxy_url) in input.account_ids.iter().zip(&input.proxy_urls) {
        let record = find_account(&state, account_id)?;
        let previous = state
            .vault
            .load(&record.secret_ref)
            .map_err(vault_error)?
            .ok_or_else(|| {
                ManagementError::not_found("account_secret_missing", "account secret missing")
            })?;
        let mut credential: AccountCredential = serde_json::from_str(&previous).map_err(|_| {
            ManagementError::internal("account_secret_invalid", "account secret is invalid")
        })?;
        credential.proxy_url = Some(normalize_proxy(proxy_url)?);
        let next = serde_json::to_string(&credential).map_err(|_| {
            ManagementError::internal(
                "account_secret_serialize",
                "account secret could not be saved",
            )
        })?;
        updates.push((record.secret_ref, previous, next));
    }
    for index in 0..updates.len() {
        if let Err(error) = state.vault.save(&updates[index].0, &updates[index].2) {
            restore_account_proxy_secrets(&state, &updates[..index])?;
            return Err(vault_error(error));
        }
    }
    if let Err(error) = state.rebuild_runtime().await {
        restore_account_proxy_secrets(&state, &updates)?;
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(Json(ProxyAssignmentResult {
        assigned: updates.len(),
        unused: input.proxy_urls.len().saturating_sub(updates.len()),
    }))
}

fn restore_account_proxy_secrets(
    state: &AppState,
    updates: &[(String, String, String)],
) -> Result<(), ManagementError> {
    for (secret_ref, previous, _) in updates {
        state
            .vault
            .save(secret_ref, previous)
            .map_err(vault_error)?;
    }
    Ok(())
}

fn restore_common_proxy(
    state: &AppState,
    configured: bool,
    value: Option<&str>,
) -> Result<(), ManagementError> {
    save_optional_proxy(state, COMMON_PROXY_SECRET_REF, value)?;
    state
        .store
        .set_common_proxy_configured(configured)
        .map_err(store_error)
}

fn save_optional_proxy(
    state: &AppState,
    secret_ref: &str,
    value: Option<&str>,
) -> Result<(), ManagementError> {
    match value {
        Some(value) => state.vault.save(secret_ref, value).map(|_| ()),
        None => state.vault.delete(secret_ref).map(|_| ()),
    }
    .map_err(vault_error)
}

fn normalize_optional_proxy(value: Option<String>) -> Result<Option<String>, ManagementError> {
    value.map(|value| normalize_proxy(&value)).transpose()
}

fn normalize_proxy(value: &str) -> Result<String, ManagementError> {
    normalize_proxy_url(value)
        .map_err(|message| ManagementError::validation("proxy_invalid", message))
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountImportInput {
    label: String,
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_at_ms: Option<u64>,
    plan_type: Option<String>,
    subscription_active_until_ms: Option<u64>,
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
    plan_type: Option<String>,
    subscription_active_until_ms: Option<u64>,
    allowed_models: Vec<String>,
    excluded_models: Vec<String>,
    priority: i32,
    weight: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    batch_session_id: Option<String>,
}

pub async fn preview_account_import(
    State(state): State<Arc<AppState>>,
    Json(input): Json<AccountImportInput>,
) -> Result<(StatusCode, Json<AccountImportPreview>), ManagementError> {
    cleanup_expired_imports(&state)?;
    let preview = prepare_account_import(&state, input, None)?;
    Ok((StatusCode::CREATED, Json(preview)))
}

fn prepare_account_import(
    state: &AppState,
    input: AccountImportInput,
    batch_session_id: Option<&str>,
) -> Result<AccountImportPreview, ManagementError> {
    validate_secret(&input.access_token, "access token")?;
    if let Some(value) = input.refresh_token.as_deref() {
        validate_secret(value, "refresh token")?;
    }
    if let Some(value) = input.id_token.as_deref() {
        validate_secret(value, "ID token")?;
    }
    let label = redact_import_label(
        clean_label(&input.label, "account label")?,
        &[
            Some(input.access_token.as_str()),
            input.refresh_token.as_deref(),
            input.id_token.as_deref(),
            Some(input.chatgpt_account_id.as_str()),
        ],
    );
    let plan_type = input
        .plan_type
        .as_deref()
        .filter(|value| {
            !contains_sensitive(
                value,
                &[
                    Some(input.access_token.as_str()),
                    input.refresh_token.as_deref(),
                    input.id_token.as_deref(),
                    Some(input.chatgpt_account_id.as_str()),
                ],
            )
        })
        .map(str::to_string)
        .and_then(safe_plan_type);
    let chatgpt_account_id = clean_identifier(&input.chatgpt_account_id, "account id")?;
    let responses_url = validate_account_responses_url(input.responses_url.as_deref())?;
    let identity_hint = identity_hint(&chatgpt_account_id);
    let duplicate_account = state
        .store
        .accounts()
        .map_err(store_error)?
        .into_iter()
        .find(|record| record.identity_hint == identity_hint);
    let duplicate_account_id = duplicate_account.as_ref().map(|record| record.id.clone());
    let account_id = duplicate_account_id
        .clone()
        .unwrap_or_else(|| format!("account_{}", uuid::Uuid::new_v4().simple()));
    let session_id = format!("import_{}", uuid::Uuid::new_v4().simple());
    let secret_ref = format!("account:{account_id}:{}", uuid::Uuid::new_v4().simple());
    let existing_credential = match duplicate_account.as_ref() {
        Some(record) => match state.vault.load(&record.secret_ref).map_err(vault_error)? {
            Some(value) => Some(serde_json::from_str::<AccountCredential>(&value).map_err(
                |_| {
                    ManagementError::internal("account_secret_invalid", "account secret is invalid")
                },
            )?),
            None => None,
        },
        None => None,
    };
    let proxy_url = existing_credential
        .as_ref()
        .and_then(|credential| credential.proxy_url.clone());
    let credential = AccountCredential {
        access_token: input.access_token,
        refresh_token: nonempty(input.refresh_token).or_else(|| {
            existing_credential
                .as_ref()
                .and_then(|credential| credential.refresh_token.clone())
        }),
        id_token: nonempty(input.id_token),
        expires_at_ms: input.expires_at_ms,
        issued_at_ms: now_ms(),
        generation: 0,
        chatgpt_account_id,
        responses_url,
        proxy_url,
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
        plan_type,
        subscription_active_until_ms: input.subscription_active_until_ms,
        allowed_models: normalized_values(input.allowed_models),
        excluded_models: normalized_values(input.excluded_models),
        priority: input.priority,
        weight: valid_weight(input.weight)?,
        batch_session_id: batch_session_id.map(str::to_string),
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
    Ok(preview)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchImportPreviewInput {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    documents: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchImportSession {
    session_id: String,
    prepared: bool,
    preview: BatchImportPreview,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchImportPreview {
    format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    rows: Vec<BatchImportRow>,
    warnings: Vec<BatchImportWarning>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchImportRow {
    item_id: String,
    label: String,
    identity: String,
    auth_mode: String,
    source_name: String,
    quota_status: String,
    status: String,
    plan: Option<String>,
    default_selected: bool,
    selectable: bool,
    existing: bool,
    warnings: Vec<BatchImportWarning>,
    error: Option<BatchImportIssue>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchImportWarning {
    code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    count: Option<usize>,
}

type ParsedBatchImport = (String, Vec<Value>, Vec<BatchImportWarning>, Option<String>);

#[derive(Serialize)]
struct BatchImportIssue {
    code: String,
    message: String,
}

pub async fn preview_account_batch_import(
    State(state): State<Arc<AppState>>,
    Json(input): Json<BatchImportPreviewInput>,
) -> Result<(StatusCode, Json<BatchImportSession>), ManagementError> {
    cleanup_expired_imports(&state)?;
    let (format, values, warnings, description) = parse_batch_import_input(input)?;
    let session_id = format!("batch_{}", uuid::Uuid::new_v4().simple());
    let mut rows = Vec::with_capacity(values.len());
    for (ordinal, value) in values.into_iter().enumerate() {
        let row = match normalize_batch_account(value) {
            Ok(input) => match prepare_account_import(&state, input, Some(&session_id)) {
                Ok(preview) => BatchImportRow {
                    item_id: preview.session_id.clone(),
                    label: preview.label,
                    identity: preview.identity_hint,
                    auth_mode: "imported_token".to_string(),
                    source_name: "OpenAI".to_string(),
                    quota_status: "skipped".to_string(),
                    status: if preview.duplicate_account_id.is_some() {
                        "existing".to_string()
                    } else {
                        "ready".to_string()
                    },
                    plan: preview.plan_type,
                    default_selected: preview.duplicate_account_id.is_none(),
                    selectable: true,
                    existing: preview.duplicate_account_id.is_some(),
                    warnings: Vec::new(),
                    error: None,
                },
                Err(error) => invalid_batch_row(ordinal, error.code, error.message),
            },
            Err((code, message)) => invalid_batch_row(ordinal, code, message),
        };
        rows.push(row);
    }
    Ok((
        StatusCode::CREATED,
        Json(BatchImportSession {
            session_id,
            prepared: true,
            preview: BatchImportPreview {
                format,
                description,
                rows,
                warnings,
            },
        }),
    ))
}

fn parse_batch_import_input(
    input: BatchImportPreviewInput,
) -> Result<ParsedBatchImport, ManagementError> {
    let content = input.content.filter(|value| !value.trim().is_empty());
    if input.documents.is_empty() {
        return parse_batch_import(content.as_deref().unwrap_or_default());
    }
    if content.is_some() {
        return Err(ManagementError::validation(
            "import_input_conflict",
            "paste content and file documents cannot be imported together",
        ));
    }
    if input.documents.len() > MAX_IMPORT_ITEMS {
        return Err(ManagementError::validation(
            "import_item_count",
            format!("import must contain between 1 and {MAX_IMPORT_ITEMS} items"),
        ));
    }
    if input.documents.len() == 1 {
        return parse_batch_import(&input.documents[0]);
    }

    let mut total_bytes = 0usize;
    let mut values = Vec::new();
    let mut warnings = Vec::new();
    for document in input.documents {
        total_bytes = total_bytes.checked_add(document.len()).ok_or_else(|| {
            ManagementError::validation("import_too_large", "import input exceeds the size limit")
        })?;
        if total_bytes > MAX_IMPORT_BYTES {
            return Err(ManagementError::validation(
                "import_too_large",
                "import input exceeds the size limit",
            ));
        }
        let (_, mut document_values, mut document_warnings, _) = match parse_batch_import(&document)
        {
            Ok(parsed) => parsed,
            Err(error) if matches!(error.code.as_str(), "import_empty" | "import_malformed") => {
                values.push(malformed_import_value());
                if values.len() > MAX_IMPORT_ITEMS {
                    return Err(ManagementError::validation(
                        "import_item_count",
                        format!("import must contain between 1 and {MAX_IMPORT_ITEMS} items"),
                    ));
                }
                continue;
            }
            Err(error) => return Err(error),
        };
        values.append(&mut document_values);
        warnings.append(&mut document_warnings);
        if values.len() > MAX_IMPORT_ITEMS {
            return Err(ManagementError::validation(
                "import_item_count",
                format!("import must contain between 1 and {MAX_IMPORT_ITEMS} items"),
            ));
        }
    }
    Ok(("json_documents".to_string(), values, warnings, None))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchImportConfirmInput {
    session_id: String,
    selected_item_ids: Vec<String>,
    #[serde(default)]
    add_to_pool: bool,
    #[serde(default)]
    probe_metadata: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchImportConfirmResponse {
    session_id: String,
    results: Vec<BatchImportResult>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchImportResult {
    item_id: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<BatchImportIssue>,
}

pub async fn confirm_account_batch_import(
    State(state): State<Arc<AppState>>,
    Json(input): Json<BatchImportConfirmInput>,
) -> Result<Json<BatchImportConfirmResponse>, ManagementError> {
    if !valid_generated_id(&input.session_id, "batch_") {
        return Err(ManagementError::validation(
            "import_session_invalid",
            "batch import session is invalid",
        ));
    }
    if input.selected_item_ids.is_empty() || input.selected_item_ids.len() > MAX_IMPORT_ITEMS {
        return Err(ManagementError::validation(
            "import_selection_invalid",
            format!("import selection must contain between 1 and {MAX_IMPORT_ITEMS} items"),
        ));
    }
    let probe_metadata =
        input.probe_metadata && input.selected_item_ids.len() <= MAX_SYNCHRONOUS_IMPORT_PROBES;
    let mut seen = HashSet::new();
    let mut results = Vec::with_capacity(input.selected_item_ids.len());
    for item_id in input.selected_item_ids {
        if !seen.insert(item_id.clone()) {
            continue;
        }
        let result = match confirm_one_account_import(
            &state,
            &item_id,
            Some(&input.session_id),
            input.add_to_pool,
            probe_metadata,
        )
        .await
        {
            Ok(account) => BatchImportResult {
                item_id,
                status: "succeeded".to_string(),
                account_id: Some(account.id),
                error: None,
            },
            Err(error) => BatchImportResult {
                item_id,
                status: "failed".to_string(),
                account_id: None,
                error: Some(BatchImportIssue {
                    code: error.code,
                    message: error.message,
                }),
            },
        };
        results.push(result);
    }
    Ok(Json(BatchImportConfirmResponse {
        session_id: input.session_id,
        results,
    }))
}

fn parse_batch_import(content: &str) -> Result<ParsedBatchImport, ManagementError> {
    if content.trim().is_empty() {
        return Err(ManagementError::validation(
            "import_empty",
            "import input is empty",
        ));
    }
    if content.len() > MAX_IMPORT_BYTES {
        return Err(ManagementError::validation(
            "import_too_large",
            "import input exceeds the size limit",
        ));
    }
    let parsed = serde_json::from_str::<Value>(content);
    let (format, values, mut warnings, description) = match parsed {
        Ok(value) => {
            ensure_import_depth(&value, 0)?;
            match value {
                Value::Array(values) => ("json_array".to_string(), values, Vec::new(), None),
                Value::Object(mut object) => {
                    let is_zenith = object
                        .get("format")
                        .and_then(Value::as_str)
                        .is_some_and(|format| format.eq_ignore_ascii_case("zenith"));
                    if is_zenith && !object.get("accounts").is_some_and(Value::is_array) {
                        return Err(ManagementError::validation(
                            "import_malformed",
                            "Zenith account bundle has no account list",
                        ));
                    }
                    if let Some(Value::Array(accounts)) = object.remove("accounts") {
                        let version = object
                            .get("version")
                            .and_then(|value| {
                                value.as_u64().or_else(|| value.as_str()?.parse().ok())
                            })
                            .or((!is_zenith).then_some(1))
                            .ok_or_else(|| {
                                ManagementError::validation(
                                    "import_malformed",
                                    "Zenith account bundle version is missing",
                                )
                            })?;
                        if version != 1 {
                            return Err(ManagementError::validation(
                                "unsupported_bundle_version",
                                if is_zenith {
                                    "Zenith account bundle version is unsupported"
                                } else {
                                    "portable account bundle version is unsupported"
                                },
                            ));
                        }
                        if is_zenith {
                            if accounts.is_empty() {
                                return Err(ManagementError::validation(
                                    "import_item_count",
                                    "Zenith account bundle has no accounts",
                                ));
                            }
                            let description = match object.get("description") {
                                None | Some(Value::Null) => None,
                                Some(Value::String(description)) => {
                                    normalize_account_export_description(Some(description))
                                        .map_err(|_| {
                                            ManagementError::validation(
                                                "import_description_invalid",
                                                "Zenith account bundle description is invalid",
                                            )
                                        })?
                                        .map(str::to_string)
                                }
                                Some(_) => {
                                    return Err(ManagementError::validation(
                                        "import_description_invalid",
                                        "Zenith account bundle description is invalid",
                                    ));
                                }
                            };
                            ("zenith_v1".to_string(), accounts, Vec::new(), description)
                        } else {
                            let mut warnings = Vec::new();
                            for (key, code) in [
                                ("proxies", "proxies_ignored"),
                                ("sources", "sources_ignored"),
                            ] {
                                let count = object.get(key).map(container_count).unwrap_or(0);
                                if count > 0 {
                                    warnings.push(BatchImportWarning {
                                        code: code.to_string(),
                                        count: Some(count),
                                    });
                                }
                            }
                            (
                                "portable_account_bundle".to_string(),
                                accounts,
                                warnings,
                                None,
                            )
                        }
                    } else {
                        (
                            "json_object".to_string(),
                            vec![Value::Object(object)],
                            Vec::new(),
                            None,
                        )
                    }
                }
                Value::String(value) => match raw_access_token(&value) {
                    Some(token) => (
                        "json_object".to_string(),
                        vec![access_token_value(token)],
                        Vec::new(),
                        None,
                    ),
                    None => {
                        return Err(ManagementError::validation(
                            "import_unsupported",
                            "import input must contain JSON objects or access tokens",
                        ));
                    }
                },
                _ => {
                    return Err(ManagementError::validation(
                        "import_unsupported",
                        "import input must contain JSON objects or access tokens",
                    ));
                }
            }
        }
        Err(_) => {
            let lines = content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .collect::<Vec<_>>();
            let multiple = lines.len() > 1;
            let mut values = Vec::new();
            for line in lines {
                match serde_json::from_str::<Value>(line) {
                    Ok(value) => {
                        ensure_import_depth(&value, 0)?;
                        values.push(value);
                    }
                    Err(_) => match raw_access_token(line) {
                        Some(token) => values.push(access_token_value(token)),
                        None if multiple => values.push(malformed_import_value()),
                        None => {
                            return Err(ManagementError::validation(
                                "import_malformed",
                                "import input is not valid JSON or an access token",
                            ));
                        }
                    },
                }
            }
            ("json_lines".to_string(), values, Vec::new(), None)
        }
    };
    let values = values
        .into_iter()
        .map(normalize_token_value)
        .collect::<Vec<_>>();
    if values.is_empty() || values.len() > MAX_IMPORT_ITEMS {
        return Err(ManagementError::validation(
            "import_item_count",
            format!("import must contain between 1 and {MAX_IMPORT_ITEMS} items"),
        ));
    }
    warnings.shrink_to_fit();
    Ok((format, values, warnings, description))
}

fn normalize_token_value(value: Value) -> Value {
    match value {
        Value::String(value) => raw_access_token(&value)
            .map(access_token_value)
            .unwrap_or(Value::String(value)),
        value => value,
    }
}

fn raw_access_token(value: &str) -> Option<&str> {
    let value = value.trim();
    let token = value
        .get(..7)
        .filter(|prefix| prefix.eq_ignore_ascii_case("bearer "))
        .and_then(|_| value.get(7..))
        .map(str::trim)
        .unwrap_or(value);
    if token.is_empty()
        || token.len() > MAX_SECRET_BYTES
        || !token.is_ascii()
        || token
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return None;
    }
    let mut parts = token.split('.');
    let jwt = matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(header), Some(payload), Some(signature), None)
            if !header.is_empty() && !payload.is_empty() && !signature.is_empty()
    );
    (jwt || token
        .strip_prefix("at-")
        .is_some_and(|value| !value.is_empty()))
    .then_some(token)
}

fn access_token_value(token: &str) -> Value {
    serde_json::json!({ "access_token": token })
}

fn malformed_import_value() -> Value {
    serde_json::json!({ IMPORT_ERROR_MARKER: true })
}

fn normalize_batch_account(value: Value) -> Result<AccountImportInput, (String, String)> {
    if value
        .as_object()
        .and_then(|object| object.get(IMPORT_ERROR_MARKER))
        .is_some_and(Value::is_boolean)
    {
        return Err(invalid_import(
            "malformed_json",
            "import file or line is malformed",
        ));
    }
    if let Ok(mut input) = serde_json::from_value::<AccountImportInput>(value.clone()) {
        let jwt = imported_jwt_metadata(input.id_token.as_deref(), Some(&input.access_token));
        input.expires_at_ms = input.expires_at_ms.or(jwt.expires_at_ms);
        input.plan_type = input.plan_type.and_then(safe_plan_type).or(jwt.plan_type);
        input.subscription_active_until_ms = input
            .subscription_active_until_ms
            .or(jwt.subscription_active_until_ms);
        return Ok(input);
    }
    let object = value
        .as_object()
        .ok_or_else(|| invalid_import("unsupported_value", "import item must be a JSON object"))?;
    let identity = object.get("identity").and_then(Value::as_object);
    let subscription = object.get("subscription").and_then(Value::as_object);
    let credentials = object
        .get("credentials")
        .and_then(Value::as_object)
        .or_else(|| object.get("auth").and_then(Value::as_object))
        .unwrap_or(object);
    let tokens = credentials
        .get("tokens")
        .and_then(Value::as_object)
        .or_else(|| object.get("tokens").and_then(Value::as_object));
    let access_token = import_string(
        object,
        credentials,
        tokens,
        &["access_token", "accessToken"],
    )
    .ok_or_else(|| {
        invalid_import(
            "missing_credentials",
            "account import requires an access token",
        )
    })?;
    let label = import_string(object, credentials, tokens, &["label", "name"])
        .unwrap_or_else(|| "Imported account".to_string());
    let refresh_token = import_string(
        object,
        credentials,
        tokens,
        &["refresh_token", "refreshToken"],
    );
    let id_token = import_string(object, credentials, tokens, &["id_token", "idToken"]);
    let jwt = imported_jwt_metadata(id_token.as_deref(), Some(&access_token));
    let expires_at_ms = import_timestamp_ms(
        object,
        credentials,
        tokens,
        &["expires_at_ms", "expiresAtMs", "expires_at", "expiresAt"],
    )
    .or(jwt.expires_at_ms);
    let plan_type = import_string(
        object,
        credentials,
        tokens,
        &[
            "chatgpt_plan_type",
            "chatgptPlanType",
            "plan_type",
            "planType",
            "plan",
        ],
    )
    .or_else(|| {
        subscription.and_then(|subscription| {
            import_string(
                subscription,
                subscription,
                None,
                &["plan", "planType", "plan_type"],
            )
        })
    })
    .filter(|value| {
        ![
            Some(access_token.as_str()),
            refresh_token.as_deref(),
            id_token.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|secret| secret == value)
    })
    .and_then(safe_plan_type)
    .or(jwt.plan_type);
    let chatgpt_account_id = import_string(
        object,
        credentials,
        tokens,
        &[
            "chatgpt_account_id",
            "chatgptAccountId",
            "account_id",
            "accountId",
        ],
    )
    .or_else(|| {
        identity.and_then(|identity| {
            import_string(
                identity,
                identity,
                None,
                &[
                    "accountId",
                    "account_id",
                    "chatgptAccountId",
                    "chatgpt_account_id",
                ],
            )
        })
    })
    .or(jwt.account_id)
    .ok_or_else(|| {
        invalid_import(
            "missing_account_id",
            "account import requires an account id",
        )
    })?;
    Ok(AccountImportInput {
        label,
        access_token,
        refresh_token,
        id_token,
        expires_at_ms,
        plan_type,
        subscription_active_until_ms: import_timestamp_ms(
            object,
            credentials,
            tokens,
            &[
                "subscription_expires_at",
                "subscriptionExpiresAt",
                "subscription_active_until",
                "subscriptionActiveUntil",
                "chatgpt_subscription_active_until",
                "chatgptSubscriptionActiveUntil",
            ],
        )
        .or_else(|| {
            subscription.and_then(|subscription| {
                import_timestamp_ms(
                    subscription,
                    subscription,
                    None,
                    &["expiresAt", "expires_at"],
                )
            })
        })
        .or(jwt.subscription_active_until_ms),
        chatgpt_account_id,
        responses_url: import_string(
            object,
            credentials,
            tokens,
            &["responses_url", "responsesUrl"],
        ),
        models: import_strings(object, "models"),
        allowed_models: import_strings(object, "allowedModels")
            .into_iter()
            .chain(import_strings(object, "allowed_models"))
            .collect(),
        excluded_models: import_strings(object, "excludedModels")
            .into_iter()
            .chain(import_strings(object, "excluded_models"))
            .collect(),
        priority: object
            .get("priority")
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or_default(),
        weight: object
            .get("weight")
            .and_then(Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .unwrap_or_else(default_weight),
    })
}

fn import_string(
    root: &Map<String, Value>,
    credentials: &Map<String, Value>,
    tokens: Option<&Map<String, Value>>,
    names: &[&str],
) -> Option<String> {
    [Some(root), Some(credentials), tokens]
        .into_iter()
        .flatten()
        .find_map(|object| {
            names.iter().find_map(|name| {
                object
                    .get(*name)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            })
        })
}

fn import_timestamp_ms(
    root: &Map<String, Value>,
    credentials: &Map<String, Value>,
    tokens: Option<&Map<String, Value>>,
    names: &[&str],
) -> Option<u64> {
    [Some(root), Some(credentials), tokens]
        .into_iter()
        .flatten()
        .find_map(|object| {
            names.iter().find_map(|name| {
                let value = object.get(*name)?;
                parse_subscription_timestamp_ms(value)
            })
        })
}

#[derive(Default)]
struct ImportedJwtMetadata {
    account_id: Option<String>,
    plan_type: Option<String>,
    subscription_active_until_ms: Option<u64>,
    expires_at_ms: Option<u64>,
}

fn imported_jwt_metadata(
    id_token: Option<&str>,
    access_token: Option<&str>,
) -> ImportedJwtMetadata {
    let mut metadata = ImportedJwtMetadata::default();
    for token in [id_token, access_token].into_iter().flatten() {
        let Some(claims) = decode_import_jwt(token) else {
            continue;
        };
        let auth = claims
            .get("https://api.openai.com/auth")
            .and_then(Value::as_object);
        metadata.account_id = metadata.account_id.or_else(|| {
            auth.and_then(|value| import_claim_string(value, &["chatgpt_account_id", "account_id"]))
        });
        metadata.plan_type = metadata.plan_type.or_else(|| {
            auth.and_then(|value| import_claim_string(value, &["chatgpt_plan_type"]))
                .and_then(safe_plan_type)
        });
        metadata.subscription_active_until_ms =
            metadata.subscription_active_until_ms.or_else(|| {
                auth.and_then(|value| value.get("chatgpt_subscription_active_until"))
                    .and_then(parse_subscription_timestamp_ms)
            });
        metadata.expires_at_ms = metadata
            .expires_at_ms
            .or_else(|| claims.get("exp").and_then(parse_subscription_timestamp_ms));
    }
    metadata
}

fn decode_import_jwt(token: &str) -> Option<Value> {
    if token.is_empty() || token.len() > MAX_JWT_BYTES {
        return None;
    }
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if header.is_empty() || payload.is_empty() || signature.is_empty() {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    if decoded.len() > MAX_JWT_PAYLOAD_BYTES {
        return None;
    }
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    value.is_object().then_some(value)
}

fn import_claim_string(object: &Map<String, Value>, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|name| object.get(*name).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= 512)
        .map(str::to_string)
}

fn import_strings(root: &Map<String, Value>, name: &str) -> Vec<String> {
    root.get(name)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn ensure_import_depth(value: &Value, depth: usize) -> Result<(), ManagementError> {
    if depth > MAX_IMPORT_DEPTH {
        return Err(ManagementError::validation(
            "import_too_deep",
            "import JSON is too deeply nested",
        ));
    }
    match value {
        Value::Array(values) => values
            .iter()
            .try_for_each(|value| ensure_import_depth(value, depth + 1)),
        Value::Object(values) => values
            .values()
            .try_for_each(|value| ensure_import_depth(value, depth + 1)),
        _ => Ok(()),
    }
}

fn container_count(value: &Value) -> usize {
    match value {
        Value::Array(values) => values.len(),
        Value::Object(values) => values.len(),
        Value::Null => 0,
        _ => 1,
    }
}

fn invalid_import(code: &str, message: &str) -> (String, String) {
    (code.to_string(), message.to_string())
}

fn invalid_batch_row(ordinal: usize, code: String, message: String) -> BatchImportRow {
    BatchImportRow {
        item_id: format!("invalid_{ordinal}"),
        label: format!("Item {}", ordinal + 1),
        identity: "••••".to_string(),
        auth_mode: "unknown".to_string(),
        source_name: "import".to_string(),
        quota_status: "skipped".to_string(),
        status: "invalid".to_string(),
        plan: None,
        default_selected: false,
        selectable: false,
        existing: false,
        warnings: Vec::new(),
        error: Some(BatchImportIssue { code, message }),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmImportInput {
    session_id: String,
    #[serde(default)]
    add_to_pool: bool,
    #[serde(default)]
    probe_metadata: bool,
}

pub async fn confirm_account_import(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ConfirmImportInput>,
) -> Result<Json<AccountSummary>, ManagementError> {
    confirm_one_account_import(
        &state,
        &input.session_id,
        None,
        input.add_to_pool,
        input.probe_metadata,
    )
    .await
    .map(Json)
}

async fn confirm_one_account_import(
    state: &Arc<AppState>,
    session_id: &str,
    batch_session_id: Option<&str>,
    add_to_pool: bool,
    probe_metadata: bool,
) -> Result<AccountSummary, ManagementError> {
    if !valid_generated_id(session_id, "import_") {
        return Err(ManagementError::validation(
            "import_session_invalid",
            "account import session is invalid",
        ));
    }
    let pending = state
        .store
        .pending_import(session_id)
        .map_err(store_error)?
        .ok_or_else(|| {
            ManagementError::not_found("import_not_found", "import session not found")
        })?;
    if now_ms().saturating_sub(pending.created_at_ms) > 30 * 60 * 1_000 {
        let _ = state.store.delete_pending_import(session_id);
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
    if preview.batch_session_id.as_deref() != batch_session_id {
        return Err(ManagementError::not_found(
            "import_not_found",
            "import session not found",
        ));
    }
    let existing = state
        .store
        .accounts()
        .map_err(store_error)?
        .into_iter()
        .find(|record| record.id == preview.account_id);
    let mut subscription =
        if preview.plan_type.is_some() || preview.subscription_active_until_ms.is_some() {
            Subscription::normalize(SubscriptionInput {
                plan_type: preview.plan_type.clone(),
                active_until_ms: preview.subscription_active_until_ms,
                forbidden: false,
                observed_at_ms: now_ms(),
            })
        } else {
            existing
                .as_ref()
                .map(|value| value.subscription.clone())
                .unwrap_or_default()
        };
    if subscription.active_until_ms.is_none() {
        subscription.updated_at_ms = None;
    }
    let mut record = ServerAccountRecord {
        id: preview.account_id.clone(),
        label: existing
            .as_ref()
            .map(|value| value.label.clone())
            .unwrap_or(preview.label),
        identity_hint: preview.identity_hint,
        enabled: existing.as_ref().is_none_or(|value| value.enabled),
        in_pool: add_to_pool || existing.as_ref().is_some_and(|value| value.in_pool),
        draining: existing.as_ref().is_some_and(|value| value.draining),
        source_id: "openai_codex".to_string(),
        secret_ref: pending.secret_ref.clone(),
        auth_state: preview.auth_state,
        health: AccountHealthState::Healthy,
        models: existing
            .as_ref()
            .map(|value| value.models.clone())
            .unwrap_or(preview.models),
        allowed_models: existing
            .as_ref()
            .map(|value| value.allowed_models.clone())
            .unwrap_or(preview.allowed_models),
        excluded_models: existing
            .as_ref()
            .map(|value| value.excluded_models.clone())
            .unwrap_or(preview.excluded_models),
        priority: existing
            .as_ref()
            .map_or(preview.priority, |value| value.priority),
        weight: existing
            .as_ref()
            .map_or(preview.weight, |value| value.weight),
        subscription,
        quota: existing
            .as_ref()
            .map(|value| value.quota.clone())
            .unwrap_or_default(),
        cooldowns: BTreeMap::new(),
        consecutive_failures: 0,
        created_at_ms: existing
            .as_ref()
            .map(|value| value.created_at_ms)
            .filter(|value| *value > 0)
            .unwrap_or(pending.created_at_ms),
        last_used_at_ms: existing.as_ref().and_then(|value| value.last_used_at_ms),
        last_error_code: None,
    };
    state.store.save_account(&record).map_err(store_error)?;
    if probe_metadata {
        match jobs::refresh_account_now(state, record.clone()).await {
            Ok(updated) => record = updated,
            Err(_) => {
                record.health = AccountHealthState::Degraded;
                record.last_error_code = Some("metadata_refresh_failed".to_string());
                let _ = state.store.save_account(&record);
            }
        }
    } else if state.rebuild_runtime().await.is_err() {
        record.health = AccountHealthState::Degraded;
        record.last_error_code = Some("runtime_rebuild_failed".to_string());
        let _ = state.store.save_account(&record);
    }
    state
        .store
        .delete_pending_import(session_id)
        .map_err(store_error)?;
    if let Some(previous) = existing {
        if previous.secret_ref != record.secret_ref {
            let _ = state.vault.delete(&previous.secret_ref);
        }
    }
    account_summary(state, &record)
}

fn cleanup_expired_imports(state: &AppState) -> Result<(), ManagementError> {
    let cutoff = now_ms().saturating_sub(30 * 60 * 1_000);
    for secret_ref in state
        .store
        .delete_pending_imports_before(cutoff)
        .map_err(store_error)?
    {
        let _ = state.vault.delete(&secret_ref);
    }
    Ok(())
}

fn valid_generated_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

fn safe_plan_type(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    .then(|| value.to_ascii_lowercase())
}

fn redact_import_label(value: String, sensitive: &[Option<&str>]) -> String {
    if contains_sensitive(&value, sensitive) {
        "Imported account".to_string()
    } else {
        value
    }
}

fn contains_sensitive(value: &str, sensitive: &[Option<&str>]) -> bool {
    sensitive
        .iter()
        .flatten()
        .any(|secret| secret.len() >= 4 && value.contains(secret))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPatch {
    label: Option<String>,
    enabled: Option<bool>,
    in_pool: Option<bool>,
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
    if let Some(value) = input.in_pool {
        record.in_pool = value;
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PoolMembershipInput {
    #[serde(default)]
    account_ids: Vec<String>,
    #[serde(default)]
    source_ids: Vec<String>,
    in_pool: bool,
}

pub async fn set_pool_membership(
    State(state): State<Arc<AppState>>,
    Json(input): Json<PoolMembershipInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let account_ids = input.account_ids.into_iter().collect::<BTreeSet<_>>();
    let source_ids = input.source_ids.into_iter().collect::<BTreeSet<_>>();
    if account_ids.is_empty() && source_ids.is_empty() {
        return Err(ManagementError::validation(
            "pool_members_empty",
            "at least one pool member is required",
        ));
    }
    if account_ids.len().saturating_add(source_ids.len()) > 2_048 {
        return Err(ManagementError::validation(
            "pool_members_too_many",
            "too many pool members were requested",
        ));
    }

    let accounts = state.store.accounts().map_err(store_error)?;
    let sources = state.store.sources().map_err(store_error)?;
    let old_accounts = account_ids
        .iter()
        .map(|id| {
            accounts
                .iter()
                .find(|record| &record.id == id)
                .map(|record| (id.clone(), record.in_pool))
                .ok_or_else(|| ManagementError::not_found("account_not_found", "account not found"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let old_sources = source_ids
        .iter()
        .map(|id| {
            sources
                .iter()
                .find(|record| &record.id == id)
                .map(|record| (id.clone(), record.in_pool))
                .ok_or_else(|| ManagementError::not_found("source_not_found", "source not found"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let next_accounts = account_ids
        .iter()
        .map(|id| (id.clone(), input.in_pool))
        .collect::<Vec<_>>();
    let next_sources = source_ids
        .iter()
        .map(|id| (id.clone(), input.in_pool))
        .collect::<Vec<_>>();
    state
        .store
        .replace_pool_membership(&next_sources, &next_accounts)
        .map_err(store_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        state
            .store
            .replace_pool_membership(&old_sources, &old_accounts)
            .map_err(|rollback| {
                ManagementError::internal(
                    "pool_membership_recovery_failed",
                    format!("{error}; failed to restore pool membership: {rollback}"),
                )
            })?;
        return Err(runtime_error(error));
    }
    state.snapshot().map(Json).map_err(store_error)
}

pub async fn refresh_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<AccountSummary>, ManagementError> {
    let record = find_account(&state, &id)?;
    let updated = jobs::refresh_account_now(&state, record)
        .await
        .map_err(|_| {
            ManagementError::new(
                StatusCode::BAD_GATEWAY,
                "account_refresh_failed",
                "account metadata could not be refreshed",
                "quota",
                true,
            )
        })?;
    account_summary(&state, &updated).map(Json)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountQuotaRefreshResult {
    refreshed: usize,
    failed: usize,
    snapshot: RuntimeStateSnapshot,
}

pub async fn refresh_all_account_quotas(
    State(state): State<Arc<AppState>>,
) -> Result<Json<AccountQuotaRefreshResult>, ManagementError> {
    let (refreshed, failed) = jobs::refresh_all_accounts_now(&state)
        .await
        .map_err(runtime_error)?;
    let snapshot = state.snapshot().map_err(store_error)?;
    Ok(Json(AccountQuotaRefreshResult {
        refreshed,
        failed,
        snapshot,
    }))
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
    let snapshot = state.snapshot().map_err(store_error)?;
    if !state.store.gateway_enabled().map_err(store_error)?
        || snapshot.gateway.candidate_count == 0
        || snapshot.gateway.visible_model_ids.is_empty()
    {
        return Err(ManagementError::new(
            StatusCode::CONFLICT,
            "profile_attach_unavailable",
            "remote gateway has no usable pool route",
            "profile_attach",
            true,
        ));
    }
    let mut key = state
        .store
        .keys()
        .map_err(store_error)?
        .into_iter()
        .find(|key| key.system)
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
            base_url: snapshot.gateway.base_url,
            secret,
        }),
    ))
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
        system: false,
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
) -> Result<Json<GeneratedKey>, ManagementError> {
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaPolicyInput {
    refresh_interval_seconds: u64,
    request_timeout_seconds: u64,
    #[serde(default)]
    use_free_accounts: Option<bool>,
}

pub async fn set_quota_policy(
    State(state): State<Arc<AppState>>,
    Json(input): Json<QuotaPolicyInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    if !(crate::store::MIN_QUOTA_REFRESH_INTERVAL_SECONDS
        ..=crate::store::MAX_QUOTA_REFRESH_INTERVAL_SECONDS)
        .contains(&input.refresh_interval_seconds)
    {
        return Err(ManagementError::validation(
            "quota_refresh_interval_invalid",
            "quota refresh interval must be between 120 and 3600 seconds",
        ));
    }
    if !(crate::store::MIN_QUOTA_REQUEST_TIMEOUT_SECONDS
        ..=crate::store::MAX_QUOTA_REQUEST_TIMEOUT_SECONDS)
        .contains(&input.request_timeout_seconds)
    {
        return Err(ManagementError::validation(
            "quota_request_timeout_invalid",
            "quota request timeout must be between 10 and 20 seconds",
        ));
    }
    let previous = state.store.quota_policy().map_err(store_error)?;
    let use_free_accounts = input.use_free_accounts.unwrap_or(previous.2);
    state
        .store
        .set_quota_policy(
            input.refresh_interval_seconds,
            input.request_timeout_seconds,
            use_free_accounts,
        )
        .map_err(store_error)?;
    if use_free_accounts != previous.2 {
        if let Err(error) = state.rebuild_runtime().await {
            state
                .store
                .set_quota_policy(previous.0, previous.1, previous.2)
                .map_err(store_error)?;
            if let Err(restore) = state.rebuild_runtime().await {
                return Err(store_error(format!(
                    "{error}; failed to restore previous runtime: {restore}"
                )));
            }
            return Err(store_error(error));
        }
    }
    state.snapshot().map(Json).map_err(store_error)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RoutingPolicyInput {
    max_retry_candidates: u8,
    #[serde(default)]
    routing_strategy: RoutingStrategy,
    #[serde(default)]
    default_service_tier: Option<DefaultServiceTier>,
    #[serde(default)]
    image_base_model: Option<Option<String>>,
    subscription_plan_order: Option<Vec<String>>,
}

pub async fn set_routing_policy(
    State(state): State<Arc<AppState>>,
    Json(input): Json<RoutingPolicyInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    if !(1..=8).contains(&input.max_retry_candidates) {
        return Err(ManagementError::validation(
            "max_retry_candidates_invalid",
            "max retry candidates must be between 1 and 8",
        ));
    }
    let previous = state.store.routing_policy().map_err(store_error)?;
    let default_service_tier = input.default_service_tier.unwrap_or(previous.2);
    let image_base_model = input.image_base_model.unwrap_or(previous.3.clone());
    let subscription_plan_order = input
        .subscription_plan_order
        .unwrap_or_else(|| previous.4.clone());
    state
        .store
        .set_routing_policy(
            input.max_retry_candidates,
            input.routing_strategy,
            default_service_tier,
            image_base_model,
            subscription_plan_order,
        )
        .map_err(store_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        state
            .store
            .set_routing_policy(previous.0, previous.1, previous.2, previous.3, previous.4)
            .map_err(store_error)?;
        if let Err(restore) = state.rebuild_runtime().await {
            return Err(store_error(format!(
                "{error}; failed to restore previous runtime: {restore}"
            )));
        }
        return Err(store_error(error));
    }
    state.snapshot().map(Json).map_err(store_error)
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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelEnabledInput {
    model_id: String,
    enabled: bool,
}

pub async fn set_model_enabled(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SetModelEnabledInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let requested = input.model_id.trim();
    if requested.is_empty() || requested.len() > 256 || requested.chars().any(char::is_control) {
        return Err(ManagementError::validation(
            "model_id_invalid",
            "model id is invalid",
        ));
    }
    let snapshot = state.snapshot().map_err(store_error)?;
    let canonical = snapshot
        .gateway
        .models
        .iter()
        .find(|model| model.id.eq_ignore_ascii_case(requested))
        .map(|model| model.id.clone())
        .ok_or_else(|| ManagementError::not_found("model_not_found", "pool model not found"))?;
    let old_hidden = state.store.hidden_models().map_err(store_error)?;
    let mut hidden = old_hidden.clone();
    hidden.retain(|model| !model.eq_ignore_ascii_case(&canonical));
    if !input.enabled {
        hidden.push(canonical);
    }
    if hidden == old_hidden {
        return Ok(Json(snapshot));
    }
    state.store.set_hidden_models(hidden).map_err(store_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        state
            .store
            .set_hidden_models(old_hidden)
            .map_err(|rollback| {
                ManagementError::internal(
                    "model_rule_recovery_failed",
                    format!("{error}; failed to restore model rules: {rollback}"),
                )
            })?;
        return Err(runtime_error(error));
    }
    state.snapshot().map(Json).map_err(store_error)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelPriceInput {
    model_id: String,
    input_micro_usd_per_million: Option<u64>,
    cached_input_micro_usd_per_million: Option<u64>,
    output_micro_usd_per_million: Option<u64>,
}

pub async fn set_model_price(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SetModelPriceInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let price = match (
        input.input_micro_usd_per_million,
        input.cached_input_micro_usd_per_million,
        input.output_micro_usd_per_million,
    ) {
        (Some(input), cached_input, Some(output))
            if input <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION
                && cached_input.unwrap_or(input) <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION
                && output <= MAX_MODEL_PRICE_MICRO_USD_PER_MILLION =>
        {
            Some(ApiModelPriceOverride {
                input_micro_usd_per_million: input,
                cached_input_micro_usd_per_million: Some(cached_input.unwrap_or(input)),
                output_micro_usd_per_million: output,
            })
        }
        (None, None, None) => None,
        _ => {
            return Err(ManagementError::validation(
                "model_price_invalid",
                "input, cached input, and output model prices must be valid",
            ));
        }
    };
    let requested = input.model_id.trim();
    if requested.is_empty() || requested.len() > 256 || requested.chars().any(char::is_control) {
        return Err(ManagementError::validation(
            "model_id_invalid",
            "model id is invalid",
        ));
    }
    let snapshot = state.snapshot().map_err(store_error)?;
    let canonical = snapshot
        .gateway
        .models
        .iter()
        .find(|model| model.id.eq_ignore_ascii_case(requested))
        .map(|model| model.id.to_ascii_lowercase())
        .ok_or_else(|| ManagementError::not_found("model_not_found", "pool model not found"))?;
    let mut overrides = state.store.model_price_overrides().map_err(store_error)?;
    if let Some(price) = price {
        overrides.insert(canonical, price);
    } else {
        overrides.remove(&canonical);
    }
    state
        .store
        .set_model_price_overrides(overrides)
        .map_err(store_error)?;
    state.snapshot().map(Json).map_err(store_error)
}

pub async fn usage(
    State(state): State<Arc<AppState>>,
    Query(mut query): Query<UsageQuery>,
) -> Result<Json<UsagePage>, ManagementError> {
    normalize_usage_query(&mut query)?;
    let mut page = state.store.usage_page(&query).map_err(store_error)?;
    let snapshot = state.snapshot().map_err(store_error)?;
    let labels = snapshot
        .accounts
        .into_iter()
        .map(|account| (identity_hint(&account.id), account.label))
        .chain(
            snapshot
                .sources
                .into_iter()
                .map(|source| (identity_hint(&source.id), source.name)),
        )
        .collect::<HashMap<_, _>>();
    for event in &mut page.events {
        event.candidate_label = labels.get(&event.candidate_hint).cloned();
    }
    for group in &mut page.pool_members {
        group.label = labels.get(&group.key).cloned();
    }
    Ok(Json(page))
}

pub async fn clear_usage(
    State(state): State<Arc<AppState>>,
) -> Result<StatusCode, ManagementError> {
    state.store.clear_usage().map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiagnosticInput {
    #[serde(default)]
    stream: bool,
}

pub async fn diagnose_gateway(
    State(state): State<Arc<AppState>>,
    Json(input): Json<DiagnosticInput>,
) -> Result<Json<GatewayDiagnostic>, ManagementError> {
    if !state.store.gateway_enabled().map_err(store_error)? {
        return Err(ManagementError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "gateway_stopped",
            "personal pool gateway is stopped",
            "diagnostics",
            true,
        ));
    }
    let runtime = state.runtime().map_err(runtime_error)?.ok_or_else(|| {
        ManagementError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "runtime_unavailable",
            "personal pool runtime is unavailable",
            "diagnostics",
            true,
        )
    })?;
    let secret = state
        .store
        .keys()
        .map_err(store_error)?
        .into_iter()
        .find_map(|key| {
            key.enabled
                .then(|| state.vault.load(&key.secret_ref).ok().flatten())
                .flatten()
        })
        .ok_or_else(|| {
            ManagementError::validation(
                "diagnostic_key_unavailable",
                "no enabled personal pool key is available",
            )
        })?;

    let models =
        internal_gateway_request(runtime.clone(), "GET", "/v1/models", &secret, Body::empty())
            .await?;
    let model = serde_json::from_slice::<Value>(&models)
        .ok()
        .and_then(|value| value.get("data").and_then(Value::as_array).cloned())
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("id").and_then(Value::as_str).map(str::to_string))
        .find(|id| valid_diagnostic_model(id))
        .ok_or_else(|| {
            ManagementError::validation(
                "diagnostic_model_unavailable",
                "personal pool key exposes no usable model",
            )
        })?;
    let body = serde_json::to_vec(&serde_json::json!({
        "model": model,
        "input": "Reply with OK.",
        "stream": input.stream,
        "max_output_tokens": 8,
        "tools": []
    }))
    .map_err(|_| ManagementError::internal("diagnostic_failed", "diagnostic request failed"))?;
    let started = Instant::now();
    let response =
        internal_gateway_request(runtime, "POST", "/v1/responses", &secret, Body::from(body))
            .await?;
    if input.stream {
        let text = std::str::from_utf8(&response).map_err(|_| {
            ManagementError::internal("diagnostic_invalid", "stream diagnostic was invalid")
        })?;
        if !text.contains("response.completed") && !text.contains("[DONE]") {
            return Err(ManagementError::internal(
                "diagnostic_incomplete",
                "stream diagnostic did not reach a terminal event",
            ));
        }
    } else if !serde_json::from_slice::<Value>(&response)
        .is_ok_and(|value| value.is_object() && value.get("error").is_none_or(Value::is_null))
    {
        return Err(ManagementError::internal(
            "diagnostic_invalid",
            "non-stream diagnostic was invalid",
        ));
    }
    Ok(Json(GatewayDiagnostic {
        stream: input.stream,
        model,
        latency_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        bytes_received: response.len(),
    }))
}

const MAX_DIAGNOSTIC_BYTES: usize = 1024 * 1024;

async fn internal_gateway_request(
    runtime: Arc<zenith_relay_core::GatewayRuntime>,
    method: &str,
    uri: &str,
    secret: &str,
    body: Body,
) -> Result<Vec<u8>, ManagementError> {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "127.0.0.1")
        .header(header::AUTHORIZATION, format!("Bearer {secret}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .map_err(|_| ManagementError::internal("diagnostic_failed", "diagnostic request failed"))?;
    let response = zenith_relay_core::gateway::router(runtime)
        .oneshot(request)
        .await
        .map_err(|_| ManagementError::internal("diagnostic_failed", "diagnostic request failed"))?;
    if !response.status().is_success() {
        return Err(ManagementError::new(
            StatusCode::BAD_GATEWAY,
            "diagnostic_upstream_failed",
            format!("diagnostic failed with HTTP {}", response.status().as_u16()),
            "diagnostics",
            true,
        ));
    }
    to_bytes(response.into_body(), MAX_DIAGNOSTIC_BYTES)
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|_| {
            ManagementError::internal(
                "diagnostic_too_large",
                "diagnostic response exceeded the limit",
            )
        })
}

fn valid_diagnostic_model(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.chars().any(char::is_control)
        && !value.chars().any(char::is_whitespace)
}

fn normalize_usage_query(query: &mut UsageQuery) -> Result<(), ManagementError> {
    query.page = query.page.max(1);
    query.page_size = if query.page_size == 0 {
        50
    } else {
        query.page_size.clamp(1, 200)
    };
    query.bucket_ms = query.bucket_ms.filter(|value| *value >= 60_000);
    for value in [
        &mut query.model_query,
        &mut query.source_or_account_query,
        &mut query.local_key_query,
        &mut query.error_category,
        &mut query.request_id_query,
    ] {
        if let Some(text) = value {
            *text = text.trim().to_string();
            if text.is_empty() {
                *value = None;
            } else if text.len() > 256 || text.chars().any(char::is_control) {
                return Err(validation_error("usage filter is invalid"));
            }
        }
    }
    let now = now_ms();
    query.from_ms = match query.range {
        Some(UsageRange::Daily) => Some(utc_calendar_day_bounds(now).0),
        Some(UsageRange::Weekly) => Some(now.saturating_sub(7 * 24 * 60 * 60 * 1_000)),
        Some(UsageRange::Monthly) => Some(now.saturating_sub(30 * 24 * 60 * 60 * 1_000)),
        Some(UsageRange::Custom) => query.from_ms,
        None => query.from_ms,
    };
    if matches!(query.range, Some(UsageRange::Daily)) {
        query.to_ms = Some(utc_calendar_day_bounds(now).1.saturating_sub(1));
    }
    if matches!(query.range, Some(UsageRange::Custom))
        && (query.from_ms.is_none() || query.to_ms.is_none())
    {
        return Err(validation_error(
            "custom usage range requires fromMs and toMs",
        ));
    }
    if query
        .from_ms
        .zip(query.to_ms)
        .is_some_and(|(from, to)| from > to)
    {
        return Err(validation_error("usage range is invalid"));
    }
    Ok(())
}

fn utc_calendar_day_bounds(now_ms: u64) -> (u64, u64) {
    const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
    let start = now_ms / DAY_MS * DAY_MS;
    (start, start.saturating_add(DAY_MS))
}

#[cfg(test)]
mod usage_range_tests {
    use super::{
        normalize_batch_account, parse_batch_import, parse_batch_import_input,
        utc_calendar_day_bounds, BatchImportPreviewInput,
    };

    #[test]
    fn daily_usage_uses_utc_calendar_day() {
        const DAY_MS: u64 = 24 * 60 * 60 * 1_000;
        let now = 20 * DAY_MS + 12_345;

        assert_eq!(utc_calendar_day_bounds(now), (20 * DAY_MS, 21 * DAY_MS));
    }

    #[test]
    fn batch_import_parses_raw_token_lines() {
        let (_, values, _, _) =
            parse_batch_import("Bearer header.payload.signature\nat-opaque-token").unwrap();

        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["access_token"], "header.payload.signature");
        assert_eq!(values[1]["access_token"], "at-opaque-token");
    }

    #[test]
    fn batch_import_keeps_valid_documents_when_one_is_malformed() {
        let (_, values, _, _) = parse_batch_import_input(BatchImportPreviewInput {
            content: None,
            documents: vec![
                r#"{"account_id":"account-one","access_token":"access-one"}"#.into(),
                r#"{"access_token":"truncated""#.into(),
            ],
        })
        .unwrap();

        assert_eq!(values.len(), 2);
        assert!(normalize_batch_account(values[0].clone()).is_ok());
        assert_eq!(
            normalize_batch_account(values[1].clone()).err().unwrap().0,
            "malformed_json"
        );
    }
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
    eligible_accounts: usize,
}

pub async fn test_wake_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<WakeTestResult>, ManagementError> {
    let task = state
        .store
        .wake_tasks()
        .map_err(store_error)?
        .into_iter()
        .find(|value| value.id == id)
        .ok_or_else(|| ManagementError::not_found("wake_task_not_found", "wake task not found"))?;
    let accounts = state.store.accounts().map_err(store_error)?;
    let mut selected = match &task.account_selector {
        AccountSelector::AllEligible => accounts
            .iter()
            .filter(|account| account.enabled && !account.draining)
            .collect::<Vec<_>>(),
        AccountSelector::AccountIds(ids) => {
            let mut selected = Vec::with_capacity(ids.len());
            for account_id in ids {
                selected.push(
                    accounts
                        .iter()
                        .find(|account| account.id == *account_id)
                        .ok_or_else(|| {
                            ManagementError::validation(
                                "wake_account_missing",
                                "wake task references an unknown account",
                            )
                        })?,
                );
            }
            selected
        }
        AccountSelector::Tags(_) => Vec::new(),
    };
    selected.retain(|account| account.enabled && !account.draining);
    if let WakeModelPolicy::Explicit(model) = &task.model_policy {
        if matches!(task.account_selector, AccountSelector::AllEligible) {
            selected.retain(|account| account_supports_model(account, model));
        } else if selected
            .iter()
            .any(|account| !account_supports_model(account, model))
        {
            return Err(ManagementError::validation(
                "wake_model_unavailable",
                "wake model is unavailable for a selected account",
            ));
        }
    } else {
        selected.retain(|account| {
            account
                .models
                .iter()
                .any(|model| account_supports_model(account, model))
        });
    }
    Ok(Json(WakeTestResult {
        task_id: id,
        status: if selected.is_empty() {
            "no_eligible_accounts"
        } else {
            "ready"
        },
        eligible_accounts: selected.len(),
    }))
}

fn account_supports_model(account: &ServerAccountRecord, model: &str) -> bool {
    account
        .models
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(model))
        && (account.allowed_models.is_empty()
            || account
                .allowed_models
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(model)))
        && !account
            .excluded_models
            .iter()
            .any(|excluded| excluded.eq_ignore_ascii_case(model))
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
        in_pool: false,
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
    if matches!(record.wire_api, WireApi::Messages) {
        return Err(ManagementError::validation(
            "source_protocol_unsupported",
            "source protocol is not supported",
        ));
    }
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

async fn discover_models(
    record: &SourceRecord,
    api_key: &str,
) -> Result<Vec<String>, ManagementError> {
    let source = ProviderSource {
        id: record.id.clone(),
        name: record.name.clone(),
        base_url: record.base_url.clone(),
        api_key: api_key.to_string(),
        wire_api: record.wire_api,
        models: record.models.clone(),
    };
    let models = discover_source_models(&source).await.map_err(|_| {
        ManagementError::new(
            StatusCode::BAD_GATEWAY,
            "source_test_failed",
            "source model discovery failed",
            "upstream",
            true,
        )
    })?;
    if models.is_empty() {
        return Err(ManagementError::validation(
            "source_models_empty",
            "source did not expose any models",
        ));
    }
    Ok(models)
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

fn ensure_not_server_self_source(
    state: &AppState,
    source_base_url: &str,
) -> Result<(), ManagementError> {
    let gateway_base_url = format!(
        "{}/v1",
        state.config.public_base_url.as_str().trim_end_matches('/')
    );
    if source_points_to_gateway(source_base_url, &gateway_base_url) {
        return Err(ManagementError::validation(
            "source_self_route",
            "source base URL must not point back to this Relay gateway",
        ));
    }
    Ok(())
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

use super::{
    restart_after_secret_change, restart_or_rollback, sync_gateway_or_rollback,
    sync_records_or_rollback,
};
use crate::{
    files::atomic_write,
    local_pool::{
        accounts::{
            credentials::CredentialStore, proxy::COMMON_PROXY_SECRET_REF, NativeSecretBackend,
        },
        error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
        models::{LocalGatewayKeyRecord, LocalPoolSnapshot},
        state::DesktopState,
        store::secret_store,
    },
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;
use zenith_relay_core::{
    protocol::{
        AccountPresetRule, ConfigurationPreset, ConfigurationPresetSettings, PresetQuotaPolicy,
        PresetRoutingPolicy, SourcePresetRule, CONFIGURATION_PRESET_FORMAT,
        CONFIGURATION_PRESET_SCHEMA_VERSION,
    },
    ApiModelPriceOverride, DefaultServiceTier, RoutingStrategy,
};

type CommandResult<T> = std::result::Result<T, CommandError>;
const SYSTEM_GATEWAY_KEY_LABEL: &str = "ChatGPT pool";

#[tauri::command]
pub fn export_local_configuration_preset(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<Option<String>> {
    let (gateway, sources, accounts) = {
        let store = state.store()?;
        (
            store.gateway().clone(),
            store.sources().to_vec(),
            store.accounts().to_vec(),
        )
    };
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let sources = sources
        .into_iter()
        .map(|record| SourcePresetRule {
            id: record.id,
            name: record.name,
            base_url: record.base_url.trim_end_matches('/').to_string(),
            wire_api: record.wire_api,
            enabled: record.enabled,
            in_pool: record.in_pool,
            allowed_models: record.allowed_models,
            excluded_models: record.excluded_models,
            priority: record.priority,
            weight: record.weight,
            recovery_delay_seconds: record.recovery_delay_seconds,
            model_price_overrides: record.model_price_overrides,
        })
        .collect();
    let accounts = accounts
        .into_iter()
        .map(|record| {
            let credential = credentials.load(&record.account.id).map_err(|error| {
                LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error.to_string())
            })?;
            let proxy_id = credential.as_ref().and_then(|credential| {
                credential
                    .proxy_url()
                    .and_then(|value| zenith_relay_core::proxy_reference_id(value).ok())
            });
            Ok(AccountPresetRule {
                id: record.account.id,
                identity_hint: record
                    .account
                    .identity
                    .identity_hash
                    .chars()
                    .take(12)
                    .collect(),
                enabled: record.account.enabled,
                in_pool: record.account.in_pool,
                allowed_models: record.allowed_models,
                excluded_models: record.excluded_models,
                priority: record.priority,
                weight: record.weight,
                proxy_id,
                bypass_common_proxy: credential
                    .is_some_and(|credential| credential.bypass_common_proxy()),
            })
        })
        .collect::<LocalResult<Vec<_>>>()?;
    let common_proxy_id = if gateway.common_proxy_configured {
        secret_store::load(COMMON_PROXY_SECRET_REF)?
            .as_deref()
            .and_then(|value| zenith_relay_core::proxy_reference_id(value).ok())
    } else {
        None
    };
    let preset = ConfigurationPreset {
        format: CONFIGURATION_PRESET_FORMAT.to_string(),
        schema_version: CONFIGURATION_PRESET_SCHEMA_VERSION,
        settings: ConfigurationPresetSettings {
            sources,
            accounts,
            routing: PresetRoutingPolicy {
                max_retry_candidates: gateway.max_retry_candidates,
                routing_strategy: gateway.routing_strategy,
                subscription_plan_order: gateway.subscription_plan_order,
                default_service_tier: gateway.default_service_tier,
                image_base_model: gateway.image_base_model,
            },
            quota: PresetQuotaPolicy {
                request_timeout_seconds: gateway.quota_request_timeout_seconds,
                account_proxy_required: gateway.account_proxy_required,
                common_proxy_id,
            },
            hidden_models: gateway.hidden_models,
            model_price_overrides: gateway.model_price_overrides,
        },
    };
    write_configuration_preset(&preset, &app)
}

pub(super) fn write_configuration_preset(
    preset: &ConfigurationPreset,
    app: &AppHandle,
) -> CommandResult<Option<String>> {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("Zenith Relay configuration", &["json"])
        .set_file_name(format!(
            "zenith-relay-configuration-{}.json",
            chrono::Utc::now().format("%Y%m%d-%H%M%S")
        ))
        .blocking_save_file()
    else {
        return Ok(None);
    };
    let path = path.into_path().map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "selected preset path is invalid")
    })?;
    let content = serde_json::to_string_pretty(preset).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "configuration preset could not be serialized",
        )
    })?;
    atomic_write(&path, &format!("{content}\n"))
        .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    Ok(Some(path.to_string_lossy().into_owned()))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedLocalKey {
    pub key: LocalGatewayKeyRecord,
    pub secret: String,
}

pub(super) fn ensure_local_gateway_key_secret(key: &LocalGatewayKeyRecord) -> LocalResult<String> {
    if let Some(secret) = secret_store::load(&key.secret_ref)? {
        return Ok(secret);
    }
    let secret = format!("zlr_{}", Uuid::new_v4().simple());
    secret_store::save(&key.secret_ref, &secret)?;
    Ok(secret)
}

pub(super) fn ensure_system_gateway_key(
    state: &DesktopState,
) -> LocalResult<LocalGatewayKeyRecord> {
    let existing = { state.store()?.keys().iter().find(|key| key.system).cloned() };
    if let Some(mut key) = existing {
        if !key.enabled {
            key.enabled = true;
            state.store()?.upsert_key(key.clone())?;
        }
        ensure_local_gateway_key_secret(&key)?;
        return Ok(key);
    }

    let id = format!("key_{}", Uuid::new_v4().simple());
    let key = LocalGatewayKeyRecord {
        secret_ref: format!("key:{id}"),
        id,
        label: SYSTEM_GATEWAY_KEY_LABEL.into(),
        enabled: true,
        system: true,
        source_ids: None,
        account_ids: None,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        model_prefix: None,
        created_at: Utc::now().to_rfc3339(),
        last_used_at: None,
    };
    ensure_local_gateway_key_secret(&key)?;
    if let Err(error) = state.store()?.upsert_key(key.clone()) {
        cleanup_created_secret(&key.secret_ref, &error)?;
        return Err(error);
    }
    Ok(key)
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
    routing_strategy: RoutingStrategy,
    subscription_plan_order: Option<Vec<String>>,
    #[serde(default)]
    default_service_tier: DefaultServiceTier,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelEnabledInput {
    model_id: String,
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelPriceInput {
    model_id: String,
    input_micro_usd_per_million: Option<u64>,
    cached_input_micro_usd_per_million: Option<u64>,
    cache_write_5m_micro_usd_per_million: Option<u64>,
    cache_write_1h_micro_usd_per_million: Option<u64>,
    output_micro_usd_per_million: Option<u64>,
}

#[tauri::command]
pub async fn set_local_model_enabled(
    input: SetModelEnabledInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let canonical = canonical_pool_model(&state, &input.model_id)?;
    let old_gateway = state.store()?.gateway().clone();
    let mut gateway = old_gateway.clone();
    gateway
        .hidden_models
        .retain(|model| !model.eq_ignore_ascii_case(&canonical));
    if !input.enabled {
        gateway.hidden_models.push(canonical);
    }
    if gateway == old_gateway {
        return state.snapshot().await.map_err(Into::into);
    }
    state.store()?.replace_gateway(gateway)?;
    sync_gateway_or_rollback(&state, old_gateway).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_local_model_price(
    input: SetModelPriceInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let price = ApiModelPriceOverride::from_optional_fields(
        input.input_micro_usd_per_million,
        input.cached_input_micro_usd_per_million,
        input.cache_write_5m_micro_usd_per_million,
        input.cache_write_1h_micro_usd_per_million,
        input.output_micro_usd_per_million,
    )
    .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    let _mutation = state.setup_guard().await;
    let canonical = canonical_pool_model(&state, &input.model_id)?;
    let old_gateway = state.store()?.gateway().clone();
    let mut gateway = old_gateway.clone();
    let key = canonical.to_ascii_lowercase();
    if let Some(price) = price {
        gateway.model_price_overrides.insert(key, price);
    } else {
        gateway.model_price_overrides.remove(&key);
    }
    if gateway != old_gateway {
        let mut store = state.store()?;
        store.replace_gateway(gateway)?;
        store.reset_quota_economics_learning()?;
    }
    state.snapshot().await.map_err(Into::into)
}

fn canonical_pool_model(state: &DesktopState, model_id: &str) -> LocalResult<String> {
    let requested = model_id.trim();
    if requested.is_empty() || requested.len() > 256 || requested.chars().any(char::is_control) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "model id is invalid",
        ));
    }
    let store = state.store()?;
    store
        .sources()
        .iter()
        .filter(|source| source.in_pool)
        .flat_map(|source| source.models.iter())
        .chain(
            store
                .accounts()
                .iter()
                .filter(|account| account.account.in_pool)
                .flat_map(|account| account.models.iter()),
        )
        .find(|model| model.eq_ignore_ascii_case(requested))
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "pool model not found"))
}

#[tauri::command]
pub async fn set_local_pool_membership(
    input: PoolMembershipInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let account_ids = input.account_ids.into_iter().collect::<BTreeSet<_>>();
    let source_ids = input.source_ids.into_iter().collect::<BTreeSet<_>>();
    if account_ids.is_empty() && source_ids.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "at least one pool member is required",
        )
        .into());
    }

    let _mutation = state.setup_guard().await;
    let (old_sources, old_accounts, old_keys) = {
        let store = state.store()?;
        (
            store.sources().to_vec(),
            store.accounts().to_vec(),
            store.keys().to_vec(),
        )
    };
    if source_ids
        .iter()
        .any(|id| !old_sources.iter().any(|record| &record.id == id))
        || account_ids
            .iter()
            .any(|id| !old_accounts.iter().any(|record| &record.account.id == id))
    {
        return Err(LocalPoolError::new(ErrorCode::NotFound, "pool member not found").into());
    }
    if input.in_pool
        && old_accounts.iter().any(|record| {
            account_ids.contains(&record.account.id) && record.remote_location.is_some()
        })
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "an account managed by a remote server cannot join the local pool",
        )
        .into());
    }

    let mut sources = old_sources.clone();
    let mut accounts = old_accounts.clone();
    for source in &mut sources {
        if source_ids.contains(&source.id) {
            source.in_pool = input.in_pool;
        }
    }
    for account in &mut accounts {
        if account_ids.contains(&account.account.id) {
            account.account.in_pool = input.in_pool;
        }
    }
    if sources == old_sources && accounts == old_accounts {
        return state.snapshot().await.map_err(Into::into);
    }

    state
        .store()?
        .replace_pool_records(sources, accounts, old_keys.clone())?;
    restart_or_rollback(&state, || {
        state
            .store()?
            .replace_pool_records(old_sources, old_accounts, old_keys)
    })
    .await?;
    let now_ms = super::current_time_ms();
    for account_id in account_ids {
        state.sync_account_quota_refresh(&account_id, now_ms)?;
    }
    state.snapshot().await.map_err(Into::into)
}

#[allow(clippy::too_many_arguments)]
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
        system: false,
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
        system: current.system,
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
    gateway.routing_strategy = input.routing_strategy;
    if let Some(subscription_plan_order) = input.subscription_plan_order {
        gateway.subscription_plan_order = subscription_plan_order;
    }
    gateway.default_service_tier = input.default_service_tier;
    if gateway == old_gateway {
        return state.snapshot().await.map_err(Into::into);
    }
    let service_tier_only = gateway.max_retry_candidates == old_gateway.max_retry_candidates
        && gateway.routing_strategy == old_gateway.routing_strategy
        && gateway.subscription_plan_order == old_gateway.subscription_plan_order;
    let default_service_tier = gateway.default_service_tier;
    state.store()?.replace_gateway(gateway)?;
    if service_tier_only {
        if let Some(runtime) = state.gateway.runtime().await {
            runtime.set_default_service_tier(default_service_tier);
        }
    } else {
        sync_gateway_or_rollback(&state, old_gateway).await?;
    }
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
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    for source in store.sources() {
        let scoped = key
            .source_ids
            .as_ref()
            .is_none_or(|ids| ids.iter().any(|id| id == &source.id));
        if scoped
            && source.enabled
            && source.in_pool
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
        if !scoped
            || !account.account.enabled
            || !account.account.in_pool
            || account.account.draining
        {
            continue;
        }
        if credentials
            .load(&account.account.id)
            .map_err(|error| {
                LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error.to_string())
            })?
            .is_some()
        {
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

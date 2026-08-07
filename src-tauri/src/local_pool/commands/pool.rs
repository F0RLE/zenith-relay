use super::{restart_or_rollback, sync_gateway_or_rollback};
use crate::{
    files::atomic_write,
    local_pool::{
        accounts::{
            credentials::CredentialStore, proxy::COMMON_PROXY_SECRET_REF, NativeSecretBackend,
        },
        error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
        models::{LocalGatewayKeyRecord, LocalPoolSnapshot, ProviderSourceRecord},
        state::DesktopState,
        store::secret_store,
    },
};
use chrono::Utc;
use serde::Deserialize;
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
    ApiModelPriceOverride, DefaultServiceTier, RoutingStrategy, WireApi,
};

type CommandResult<T> = std::result::Result<T, CommandError>;
const SYSTEM_GATEWAY_KEY_LABEL: &str = "ChatGPT pool";
const SYSTEM_GATEWAY_KEY_ID: &str = "key_system";

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
            protocol_bindings: record.protocol_bindings,
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
                cooldown_after_failures: gateway.cooldown_after_failures,
                keep_last_candidate_available: gateway.keep_last_candidate_available,
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
    let existing = {
        let keys = state.store()?.keys().to_vec();
        keys.into_iter()
            .find(|key| key.id == SYSTEM_GATEWAY_KEY_ID)
            .or_else(|| {
                state
                    .store()
                    .ok()
                    .and_then(|store| store.keys().iter().find(|key| key.system).cloned())
            })
    };
    if let Some(mut key) = existing {
        if !key.enabled || !key.system || key.label != SYSTEM_GATEWAY_KEY_LABEL {
            key.enabled = true;
            key.system = true;
            key.label = SYSTEM_GATEWAY_KEY_LABEL.into();
            state.store()?.upsert_key(key.clone())?;
        }
        ensure_local_gateway_key_secret(&key)?;
        return Ok(key);
    }

    let id = SYSTEM_GATEWAY_KEY_ID.to_string();
    let key = LocalGatewayKeyRecord {
        secret_ref: format!("key:{id}"),
        id,
        label: SYSTEM_GATEWAY_KEY_LABEL.into(),
        enabled: true,
        system: true,
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
pub(crate) fn retire_user_gateway_keys(state: &DesktopState) -> LocalResult<()> {
    let (sources, all_keys, canonical_id) = {
        let store = state.store()?;
        let keys = store.keys().to_vec();
        let canonical_id = keys
            .iter()
            .find(|key| key.id == SYSTEM_GATEWAY_KEY_ID)
            .or_else(|| keys.iter().find(|key| key.system))
            .map(|key| key.id.clone());
        (store.sources().to_vec(), keys, canonical_id)
    };
    let retired = all_keys
        .iter()
        .filter(|key| canonical_id.as_deref() != Some(key.id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let mut retained = all_keys
        .iter()
        .filter(|key| canonical_id.as_deref() == Some(key.id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let needs_normalization = retained
        .first()
        .is_some_and(|key| !key.enabled || !key.system || key.label != SYSTEM_GATEWAY_KEY_LABEL);
    if let Some(key) = retained.first_mut() {
        key.enabled = true;
        key.system = true;
        key.label = SYSTEM_GATEWAY_KEY_LABEL.into();
    }
    if retired.is_empty() && !needs_normalization {
        return Ok(());
    }
    let retained_secret_ref = retained.first().map(|key| key.secret_ref.as_str());
    let retired_secrets = load_retired_gateway_secrets(&retired, retained_secret_ref)?;
    state.store()?.replace_records(sources.clone(), retained)?;

    let mut attempted_refs = Vec::new();
    for (secret_ref, _) in &retired_secrets {
        attempted_refs.push(secret_ref.clone());
        if let Err(error) = secret_store::delete(secret_ref) {
            return Err(rollback_gateway_key_cleanup(
                state,
                sources,
                all_keys,
                &retired_secrets,
                &attempted_refs,
                error,
            ));
        }
    }
    Ok(())
}

fn load_retired_gateway_secrets(
    retired: &[LocalGatewayKeyRecord],
    retained_secret_ref: Option<&str>,
) -> LocalResult<Vec<(String, Option<String>)>> {
    let mut secret_refs = BTreeSet::new();
    let mut secrets = Vec::new();
    for key in retired {
        if retained_secret_ref == Some(key.secret_ref.as_str())
            || !secret_refs.insert(key.secret_ref.clone())
        {
            continue;
        }
        secrets.push((key.secret_ref.clone(), secret_store::load(&key.secret_ref)?));
    }
    Ok(secrets)
}

fn rollback_gateway_key_cleanup(
    state: &DesktopState,
    sources: Vec<ProviderSourceRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
    retired_secrets: &[(String, Option<String>)],
    attempted_refs: &[String],
    cause: LocalPoolError,
) -> LocalPoolError {
    let mut failures = Vec::new();
    let records_result = match state.store() {
        Ok(mut store) => store.replace_records(sources, old_keys),
        Err(error) => Err(error),
    };
    if let Err(error) = records_result {
        failures.push(format!("state restore failed: {error}"));
    }
    for (secret_ref, secret) in retired_secrets {
        if !attempted_refs.iter().any(|value| value == secret_ref) {
            continue;
        }
        if let Some(secret) = secret {
            if let Err(error) = secret_store::save(secret_ref, secret) {
                failures.push(format!("secret restore failed: {error}"));
            }
        }
    }
    if failures.is_empty() {
        cause
    } else {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "{}; legacy gateway credential cleanup rollback failed: {}",
                cause.message,
                failures.join("; ")
            ),
        )
    }
}

pub(crate) fn has_usable_pool_candidate(state: &DesktopState) -> LocalResult<bool> {
    let store = state.store()?;
    for source in store.sources() {
        if source.in_pool
            && source.enabled
            && !source.draining
            && source
                .supports_wire_api(WireApi::Responses)
                .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?
            && secret_store::load(&source.secret_ref)?.is_some()
        {
            return Ok(true);
        }
    }
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    for account in store.accounts() {
        if account.account.in_pool
            && account.account.enabled
            && !account.account.draining
            && credentials
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRoutingInput {
    max_retry_candidates: u8,
    #[serde(default)]
    cooldown_after_failures: Option<u8>,
    #[serde(default)]
    keep_last_candidate_available: Option<bool>,
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
    for source in store.sources().iter().filter(|source| source.in_pool) {
        let models = source
            .models_for_wire_api(WireApi::Responses)
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        if let Some(model) = models
            .into_iter()
            .find(|model| model.eq_ignore_ascii_case(requested))
        {
            return Ok(model);
        }
    }
    store
        .accounts()
        .iter()
        .filter(|account| account.account.in_pool)
        .flat_map(|account| account.models.iter())
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
    if input.in_pool {
        for source in old_sources
            .iter()
            .filter(|source| source_ids.contains(&source.id))
        {
            let supports_responses = source
                .supports_wire_api(WireApi::Responses)
                .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
            if !supports_responses {
                return Err(LocalPoolError::new(
                    ErrorCode::Conflict,
                    "only Responses API sources can join the local ChatGPT pool",
                )
                .into());
            }
        }
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

#[tauri::command]
pub async fn update_local_routing(
    input: UpdateRoutingInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let old_gateway = state.store()?.gateway().clone();
    let mut gateway = old_gateway.clone();
    gateway.max_retry_candidates = input.max_retry_candidates;
    if let Some(value) = input.cooldown_after_failures {
        gateway.cooldown_after_failures = value;
    }
    if let Some(value) = input.keep_last_candidate_available {
        gateway.keep_last_candidate_available = value;
    }
    gateway.routing_strategy = input.routing_strategy;
    if let Some(subscription_plan_order) = input.subscription_plan_order {
        gateway.subscription_plan_order = subscription_plan_order;
    }
    gateway.default_service_tier = input.default_service_tier;
    if gateway == old_gateway {
        return state.snapshot().await.map_err(Into::into);
    }
    let service_tier_only = gateway.max_retry_candidates == old_gateway.max_retry_candidates
        && gateway.cooldown_after_failures == old_gateway.cooldown_after_failures
        && gateway.keep_last_candidate_available == old_gateway.keep_last_candidate_available
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

use super::{
    cleanup_created_secret, restart_or_rollback, runtime_account_policy, sync_gateway_or_rollback,
};
use crate::{
    files::atomic_write,
    local_pool::{
        accounts::{
            credentials::CredentialStore, proxy::COMMON_PROXY_SECRET_REF, NativeSecretBackend,
        },
        error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
        models::{LocalGatewayKeyRecord, LocalPoolSnapshot, ProviderSourceRecord},
        profiles::codex,
        state::DesktopState,
        store::secret_store,
    },
    platform::default_codex_home,
};
use chrono::Utc;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;
#[cfg(test)]
use zenith_relay_core::WireApi;
use zenith_relay_core::{
    protocol::{
        AccountPresetRule, ConfigurationPreset, ConfigurationPresetApplyInput,
        ConfigurationPresetApplyResult, ConfigurationPresetChange, ConfigurationPresetPreview,
        ConfigurationPresetSettings, PresetQuotaPolicy, PresetRoutingPolicy, SourcePresetRule,
        CONFIGURATION_PRESET_FORMAT,
        CONFIGURATION_PRESET_SCHEMA_VERSION,
    },
    ApiModelPriceOverride, DefaultServiceTier, RoutingStrategy,
};

mod model_policy;
mod reasoning;

pub(super) use model_policy::{canonical_pool_model, local_pool_member_ids};
use reasoning::SetModelReasoningInput;

type CommandResult<T> = std::result::Result<T, CommandError>;
const SYSTEM_GATEWAY_KEY_LABEL: &str = "ChatGPT pool";
const SYSTEM_GATEWAY_KEY_ID: &str = "key_system";

#[tauri::command]
pub async fn set_local_model_reasoning(
    input: SetModelReasoningInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    reasoning::set_local_model_reasoning(input, state).await
}

#[tauri::command]
pub fn export_local_configuration_preset(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<Option<String>> {
    let preset = local_configuration_preset(&state)?;
    write_configuration_preset(&preset, &app)
}

fn local_configuration_preset(state: &DesktopState) -> CommandResult<ConfigurationPreset> {
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
            model_reasoning_allowed_levels: gateway.model_reasoning_allowed_levels,
            model_reasoning_allowed_levels_present: true,
            model_service_tier_overrides: gateway.model_service_tier_overrides,
            model_display_order: gateway.model_display_order,
            model_service_tier_overrides_present: true,
            model_display_order_present: true,
        },
    };
    Ok(preset)
}

fn local_preset_revision(preset: &ConfigurationPreset) -> CommandResult<String> {
    let bytes = serde_json::to_vec(preset).map_err(|error| {
        LocalPoolError::new(ErrorCode::InvalidState, format!("configuration preset could not be serialized: {error}"))
    })?;
    Ok(format!("cfg_local_{}", hex::encode(Sha256::digest(bytes))))
}

fn local_configuration_diff(
    before: &ConfigurationPreset,
    after: &ConfigurationPreset,
) -> CommandResult<Vec<ConfigurationPresetChange>> {
    let before = serde_json::to_value(before).map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?;
    let after = serde_json::to_value(after).map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?;
    let mut changes = Vec::new();
    diff_json("".into(), &before, &after, &mut changes);
    Ok(changes)
}

fn diff_json(
    path: String,
    before: &serde_json::Value,
    after: &serde_json::Value,
    changes: &mut Vec<ConfigurationPresetChange>,
) {
    match (before, after) {
        (serde_json::Value::Object(left), serde_json::Value::Object(right)) => {
            let keys = left.keys().chain(right.keys()).collect::<std::collections::BTreeSet<_>>();
            for key in keys {
                let child = format!("{path}/{}", key.replace('~', "~0").replace('/', "~1"));
                match (left.get(key), right.get(key)) {
                    (Some(before), Some(after)) => diff_json(child, before, after, changes),
                    (before, after) => changes.push(ConfigurationPresetChange {
                        path: child,
                        before: before.cloned().unwrap_or(serde_json::Value::Null),
                        after: after.cloned().unwrap_or(serde_json::Value::Null),
                    }),
                }
            }
        }
        _ if before != after => changes.push(ConfigurationPresetChange {
            path: if path.is_empty() { "/".into() } else { path },
            before: before.clone(),
            after: after.clone(),
        }),
        _ => {}
    }
}

#[tauri::command]
pub fn preview_local_configuration_preset(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<Option<ConfigurationPresetPreview>> {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("Zenith Relay configuration", &["json"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = path.into_path().map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "selected preset path is invalid")
    })?;
    let content = std::fs::read(&path)
        .map_err(|_| LocalPoolError::new(ErrorCode::Io, "configuration preset could not be read"))?;
    if content.len() > 1024 * 1024 {
        return Err(LocalPoolError::new(ErrorCode::InvalidState, "configuration preset is too large").into());
    }
    let preset: ConfigurationPreset = serde_json::from_slice(&content).map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "configuration preset is invalid or contains unsupported fields")
    })?;
    let current = local_configuration_preset(&state)?;
    let base_revision = local_preset_revision(&current)?;
    let changes = local_configuration_diff(&current, &preset)?;
    Ok(Some(ConfigurationPresetPreview { base_revision, preset, changes }))
}

#[tauri::command]
pub async fn apply_local_configuration_preset(
    input: ConfigurationPresetApplyInput,
    state: State<'_, DesktopState>,
) -> CommandResult<ConfigurationPresetApplyResult> {
    let _mutation = state.setup_guard().await;
    let current = local_configuration_preset(&state)?;
    let current_revision = local_preset_revision(&current)?;
    if input.base_revision != current_revision {
        return Err(LocalPoolError::new(ErrorCode::Conflict, "local configuration changed; preview the preset again").into());
    }
    let changes = local_configuration_diff(&current, &input.preset)?;
    if changes.is_empty() {
        return Ok(ConfigurationPresetApplyResult { previous_revision: current_revision.clone(), revision: current_revision, changes });
    }
    let (old_gateway, old_sources, old_accounts, old_keys) = {
        let store = state.store()?;
        (store.gateway().clone(), store.sources().to_vec(), store.accounts().to_vec(), store.keys().to_vec())
    };
    let source_rules = input.preset.settings.sources.iter().map(|rule| (rule.id.as_str(), rule)).collect::<std::collections::BTreeMap<_, _>>();
    let account_rules = input.preset.settings.accounts.iter().map(|rule| (rule.id.as_str(), rule)).collect::<std::collections::BTreeMap<_, _>>();
    if source_rules.len() != input.preset.settings.sources.len() || account_rules.len() != input.preset.settings.accounts.len() {
        return Err(LocalPoolError::new(ErrorCode::InvalidState, "configuration preset contains duplicate member ids").into());
    }
    if old_sources.iter().any(|source| !source_rules.contains_key(source.id.as_str())) || old_accounts.iter().any(|account| !account_rules.contains_key(account.account.id.as_str())) {
        return Err(LocalPoolError::new(ErrorCode::Conflict, "configuration preset must include every existing local member").into());
    }
    let mut sources = old_sources.clone();
    for source in &mut sources {
        let rule = source_rules[source.id.as_str()];
        source.name = rule.name.clone();
        source.base_url = rule.base_url.clone();
        source.wire_api = rule.wire_api;
        source.protocol_bindings = rule.protocol_bindings.clone();
        source.enabled = rule.enabled;
        source.in_pool = rule.in_pool;
        source.allowed_models = rule.allowed_models.clone();
        source.excluded_models = rule.excluded_models.clone();
        source.priority = rule.priority;
        source.weight = rule.weight.max(1);
        source.recovery_delay_seconds = rule.recovery_delay_seconds;
        source.model_price_overrides = rule.model_price_overrides.clone();
    }
    let mut accounts = old_accounts.clone();
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    for account in &mut accounts {
        let rule = account_rules[account.account.id.as_str()];
        let credential = credentials.load(&account.account.id).map_err(|error| {
            LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error.to_string())
        })?;
        let current_proxy_id = credential.as_ref().and_then(|credential| {
            credential.proxy_url().and_then(|value| zenith_relay_core::proxy_reference_id(value).ok())
        });
        let current_bypass = credential.as_ref().is_some_and(|credential| credential.bypass_common_proxy());
        if rule.proxy_id != current_proxy_id || rule.bypass_common_proxy != current_bypass {
            return Err(LocalPoolError::new(ErrorCode::Conflict, "configuration preset references a different account proxy").into());
        }
        account.account.enabled = rule.enabled;
        account.account.in_pool = rule.in_pool;
        account.allowed_models = rule.allowed_models.clone();
        account.excluded_models = rule.excluded_models.clone();
        account.priority = rule.priority;
        account.weight = rule.weight.max(1);
    }
    let settings = &input.preset.settings;
    let current_proxy_id = if old_gateway.common_proxy_configured {
        secret_store::load(COMMON_PROXY_SECRET_REF)?.as_deref().and_then(|value| zenith_relay_core::proxy_reference_id(value).ok())
    } else { None };
    if settings.quota.common_proxy_id != current_proxy_id {
        return Err(LocalPoolError::new(ErrorCode::Conflict, "configuration preset references a different common proxy").into());
    }
    let mut gateway = old_gateway.clone();
    gateway.max_retry_candidates = settings.routing.max_retry_candidates;
    gateway.cooldown_after_failures = settings.routing.cooldown_after_failures;
    gateway.keep_last_candidate_available = settings.routing.keep_last_candidate_available;
    gateway.routing_strategy = settings.routing.routing_strategy;
    gateway.subscription_plan_order = settings.routing.subscription_plan_order.clone();
    gateway.default_service_tier = settings.routing.default_service_tier;
    gateway.image_base_model = settings.routing.image_base_model.clone();
    gateway.account_proxy_required = settings.quota.account_proxy_required;
    gateway.hidden_models = settings.hidden_models.clone();
    gateway.model_price_overrides = settings.model_price_overrides.clone();
    gateway.model_reasoning_allowed_levels = settings.model_reasoning_allowed_levels.clone();
    gateway.model_service_tier_overrides = settings.model_service_tier_overrides.clone();
    gateway.model_display_order = settings.model_display_order.clone();
    state.store()?.replace_pool_records(sources, accounts, old_keys.clone())?;
    state.store()?.replace_gateway(gateway)?;
    if let Err(error) = restart_or_rollback(&state, || {
        state.store()?.replace_pool_records(old_sources, old_accounts, old_keys)?;
        state.store()?.replace_gateway(old_gateway)?;
        Ok(())
    }).await {
        return Err(error.into());
    }
    let revision = local_preset_revision(&input.preset)?;
    Ok(ConfigurationPresetApplyResult { previous_revision: current_revision, revision, changes })
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
        keys.iter()
            .find(|key| key.id == SYSTEM_GATEWAY_KEY_ID)
            .or_else(|| keys.iter().find(|key| key.system))
            .cloned()
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
        secret_ref: system_gateway_secret_ref(&id),
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

fn system_gateway_secret_ref(id: &str) -> String {
    #[cfg(test)]
    {
        format!("key:{id}:test_{}", Uuid::new_v4().simple())
    }
    #[cfg(not(test))]
    {
        format!("key:{id}")
    }
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
                .supports_any_wire_api()
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelServiceTierInput {
    model_id: String,
    service_tier: DefaultServiceTier,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetModelDisplayOrderInput {
    model_ids: Vec<String>,
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
    }
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_local_model_service_tier(
    input: SetModelServiceTierInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let canonical = canonical_pool_model(&state, &input.model_id)?;
    let snapshot = state.snapshot().await?;
    let supported = state
        .gateway
        .runtime()
        .await
        .is_some_and(|runtime| runtime.model_supports_fast_service_tier(&canonical));
    if input.service_tier == DefaultServiceTier::Fast && !supported {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "Fast is not confirmed for this model by the current upstream catalog",
        )
        .into());
    }
    let old_gateway = state.store()?.gateway().clone();
    let mut gateway = old_gateway.clone();
    let key = canonical.to_ascii_lowercase();
    gateway
        .model_service_tier_overrides
        .insert(key, input.service_tier);
    if gateway == old_gateway {
        return Ok(snapshot);
    }
    state.store()?.replace_gateway(gateway.clone())?;
    if let Some(runtime) = state.gateway.runtime().await {
        if let Err(error) =
            runtime.set_model_service_tier_overrides(gateway.model_service_tier_overrides)
        {
            state.store()?.replace_gateway(old_gateway)?;
            return Err(LocalPoolError::new(ErrorCode::InvalidState, error.to_string()).into());
        }
    }
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_local_model_display_order(
    input: SetModelDisplayOrderInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let snapshot = state.snapshot().await?;
    let inputs = state.runtime_inputs().await?;
    let mut current = std::collections::BTreeMap::new();
    for source in inputs.sources.iter().filter(|source| source.in_pool) {
        for model in &source.models {
            current
                .entry(model.to_ascii_lowercase())
                .or_insert_with(|| model.clone());
        }
    }
    for account in inputs
        .accounts
        .iter()
        .filter(|account| account.account.in_pool)
    {
        for model in account.effective_models() {
            current
                .entry(model.to_ascii_lowercase())
                .or_insert_with(|| model.clone());
        }
    }
    if input.model_ids.len() != current.len() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "model order must contain every current pool model exactly once",
        )
        .into());
    }
    let mut requested = std::collections::BTreeSet::new();
    let mut order = Vec::with_capacity(input.model_ids.len());
    for model in input.model_ids {
        let key = model.trim().to_ascii_lowercase();
        let Some(canonical) = current.get(&key) else {
            return Err(LocalPoolError::new(ErrorCode::NotFound, "pool model not found").into());
        };
        if !requested.insert(key) {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "model order contains duplicates",
            )
            .into());
        }
        order.push(canonical.clone());
    }
    if requested.len() != current.len() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "model order must contain every current pool model exactly once",
        )
        .into());
    }
    let old_gateway = state.store()?.gateway().clone();
    let mut gateway = old_gateway.clone();
    gateway.model_display_order = order;
    if gateway == old_gateway {
        return Ok(snapshot);
    }
    state.store()?.replace_gateway(gateway.clone())?;
    if let Some(runtime) = state.gateway.runtime().await {
        runtime.set_model_display_order(gateway.model_display_order);
    }
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_local_pool_membership(
    input: PoolMembershipInput,
    app: AppHandle,
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
            let supports_any_protocol = source
                .supports_any_wire_api()
                .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
            if !supports_any_protocol {
                return Err(LocalPoolError::new(
                    ErrorCode::Conflict,
                    "source must expose at least one verified API route before joining the local pool",
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

    let changed_accounts = accounts
        .iter()
        .filter(|account| account_ids.contains(&account.account.id))
        .cloned()
        .collect::<Vec<_>>();
    state
        .store()?
        .replace_pool_records(sources, accounts, old_keys.clone())?;
    let policy_now_ms = super::current_time_ms();
    let updated_in_place = if let Some(runtime) = state.gateway.runtime().await {
        changed_accounts.iter().all(|account| {
            runtime.update_account_policy(
                &account.account.id,
                runtime_account_policy(account, policy_now_ms),
            )
        }) && super::apply_local_gateway_key_scope(&state, &runtime).unwrap_or(false)
    } else {
        false
    };
    if !updated_in_place {
        restart_or_rollback(&state, || {
            state
                .store()?
                .replace_pool_records(old_sources, old_accounts, old_keys)
        })
        .await?;
    }
    let now_ms = policy_now_ms;
    for account_id in account_ids {
        state.sync_account_quota_refresh(&account_id, now_ms)?;
    }
    let snapshot = state.snapshot().await?;
    drop(_mutation);
    if updated_in_place {
        tauri::async_runtime::spawn(async move {
            let state = app.state::<DesktopState>();
            let result = super::profiles::refresh_active_codex_catalog(&state).await;
            super::record_catalog_refresh_result(&state, &result);
            let _ = app.emit("zenith-state-changed", ());
        });
    }
    Ok(snapshot)
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
        codex::sync_default_service_tier(&default_codex_home(), gateway.default_service_tier)?;
        return state.snapshot().await.map_err(Into::into);
    }
    let service_tier_only = gateway.max_retry_candidates == old_gateway.max_retry_candidates
        && gateway.cooldown_after_failures == old_gateway.cooldown_after_failures
        && gateway.keep_last_candidate_available == old_gateway.keep_last_candidate_available
        && gateway.routing_strategy == old_gateway.routing_strategy
        && gateway.subscription_plan_order == old_gateway.subscription_plan_order;
    let default_service_tier = gateway.default_service_tier;
    state.store()?.replace_gateway(gateway.clone())?;
    if service_tier_only {
        if let Some(runtime) = state.gateway.runtime().await {
            runtime.set_default_service_tier(default_service_tier);
        }
    } else {
        sync_gateway_or_rollback(&state, old_gateway.clone()).await?;
    }
    if let Err(error) =
        codex::sync_default_service_tier(&default_codex_home(), default_service_tier)
    {
        state.store()?.replace_gateway(old_gateway.clone())?;
        if service_tier_only {
            if let Some(runtime) = state.gateway.runtime().await {
                runtime.set_default_service_tier(old_gateway.default_service_tier);
            }
        } else if let Err(restore) = sync_gateway_or_rollback(&state, gateway).await {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!("{error}; failed to restore previous gateway settings: {restore}"),
            )
            .into());
        }
        return Err(error.into());
    }
    state.snapshot().await.map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str, in_pool: bool, wire_api: WireApi) -> ProviderSourceRecord {
        ProviderSourceRecord {
            id: id.into(),
            name: id.into(),
            enabled: true,
            in_pool,
            draining: false,
            base_url: "https://example.test/v1".into(),
            secret_ref: format!("source:{id}"),
            wire_api,
            protocol_bindings: Vec::new(),
            models: vec!["test-model".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: Default::default(),
            detected_model_prices: Default::default(),
            last_used_at: None,
            last_test_at: None,
            last_test_status: None,
            last_error: None,
        }
    }

    #[test]
    fn local_pool_member_ids_include_native_messages_through_the_runtime_bridge() {
        let (source_ids, account_ids) = local_pool_member_ids(
            &[
                source("responses", true, WireApi::Responses),
                source("messages", true, WireApi::Messages),
                source("outside", false, WireApi::Responses),
            ],
            &[],
        )
        .unwrap();

        assert_eq!(
            source_ids,
            BTreeSet::from(["messages".to_string(), "responses".to_string()])
        );
        assert!(account_ids.is_empty());
    }
}

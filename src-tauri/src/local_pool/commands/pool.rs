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
        models::{
            LocalAccountRecord, LocalGatewayKeyRecord, LocalPoolSnapshot, ProviderSourceRecord,
        },
        profiles::codex,
        state::DesktopState,
        store::secret_store,
    },
    platform::default_codex_home,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use uuid::Uuid;
use zenith_relay_core::{
    is_valid_model_id,
    protocol::{
        AccountPresetRule, ConfigurationPreset, ConfigurationPresetSettings, PresetQuotaPolicy,
        PresetRoutingPolicy, SourcePresetRule, CONFIGURATION_PRESET_FORMAT,
        CONFIGURATION_PRESET_SCHEMA_VERSION,
    },
    AdapterRequestContext, ApiModelPriceOverride, DefaultServiceTier, ProviderSource,
    RoutingStrategy, SourceConnector, SourceProtocolBinding, WireApi,
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
            model_reasoning_allowed_levels: gateway.model_reasoning_allowed_levels,
            model_reasoning_allowed_levels_present: true,
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
pub struct SetModelReasoningInput {
    model_id: String,
    #[serde(default)]
    allowed_levels: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProbeModelReasoningInput {
    model_id: String,
    level: String,
    #[serde(default)]
    add_successful_to_settings: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReasoningProbeSourceResult {
    pub source_id: String,
    pub source_name: String,
    pub available: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReasoningProbeResult {
    pub model_id: String,
    pub level: String,
    pub source_count: usize,
    pub available_count: usize,
    pub applied_to_settings: bool,
    pub sources: Vec<ModelReasoningProbeSourceResult>,
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
pub async fn set_local_model_reasoning(
    input: SetModelReasoningInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let canonical = canonical_pool_model(&state, &input.model_id)?;
    let policy_key = zenith_relay_core::reasoning_policy_key(&canonical);
    let mut normalized_allowed_levels =
        zenith_relay_core::normalize_model_reasoning_allowed_levels(BTreeMap::from([(
            policy_key.clone(),
            input.allowed_levels,
        )]))
        .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    let allowed_levels = normalized_allowed_levels
        .remove(&policy_key)
        .unwrap_or_default();
    let old_gateway = state.store()?.gateway().clone();
    let mut gateway = old_gateway.clone();
    gateway
        .model_reasoning_allowed_levels
        .remove(&canonical.to_ascii_lowercase());
    // Keep an explicit empty override so the user can disable every
    // provider-reported mode without losing that choice on the next refresh.
    gateway
        .model_reasoning_allowed_levels
        .insert(policy_key, allowed_levels);
    if gateway == old_gateway {
        return state.snapshot().await.map_err(Into::into);
    }
    state.store()?.replace_gateway(gateway.clone())?;
    if let Some(runtime) = state.gateway.runtime().await {
        if let Err(error) =
            runtime.set_model_reasoning_allowed_levels(gateway.model_reasoning_allowed_levels)
        {
            state.store()?.replace_gateway(old_gateway)?;
            return Err(LocalPoolError::new(ErrorCode::InvalidState, error.to_string()).into());
        }
    }
    let snapshot = state.snapshot().await?;
    drop(_mutation);
    super::refresh_active_codex_catalog_in_background(app);
    Ok(snapshot)
}

/// Sends one intentionally small Responses request through every eligible API
/// source that serves this pool model.  It uses the configured adapter so the
/// probe exercises the same provider-facing request shape as the pool.
#[tauri::command]
pub async fn probe_local_model_reasoning(
    input: ProbeModelReasoningInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<ModelReasoningProbeResult> {
    let canonical = canonical_pool_model(&state, &input.model_id)?;
    let level = normalized_reasoning_probe_level(&canonical, input.level)?;
    let sources = state.store()?.sources().to_vec();
    let mut results = Vec::new();
    let mut probes = tokio::task::JoinSet::new();

    for source in sources
        .into_iter()
        .filter(|source| source.in_pool && source.enabled && !source.draining)
    {
        let Some((binding, source_model)) = source_probe_binding(&source, &canonical) else {
            continue;
        };
        let source_id = source.id.clone();
        let source_name = source.name.clone();
        if !binding.supports_reasoning_effort(&level) {
            results.push(ModelReasoningProbeSourceResult {
                source_id,
                source_name,
                available: false,
            });
            continue;
        }
        let Ok(Some(api_key)) = secret_store::load(&source.secret_ref) else {
            results.push(ModelReasoningProbeSourceResult {
                source_id,
                source_name,
                available: false,
            });
            continue;
        };
        let probe_level = level.clone();
        probes.spawn(async move {
            probe_source_reasoning(source, api_key, binding, source_model, probe_level).await
        });
    }
    while let Some(result) = probes.join_next().await {
        if let Ok(result) = result {
            results.push(result);
        }
    }
    results.sort_by(|left, right| left.source_name.cmp(&right.source_name));
    if results.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::NotFound,
            "no eligible API source can probe this pool model",
        )
        .into());
    }
    let available_count = results.iter().filter(|result| result.available).count();
    let applied_to_settings = input.add_successful_to_settings && available_count > 0;
    if applied_to_settings {
        let gateway = state.store()?.gateway().clone();
        let mut allowed_levels = zenith_relay_core::reasoning_policy_levels(
            &gateway.model_reasoning_allowed_levels,
            &canonical,
        )
        .map(ToOwned::to_owned)
        .unwrap_or_default();
        allowed_levels.push(level.clone());
        set_local_model_reasoning(
            SetModelReasoningInput {
                model_id: canonical.clone(),
                allowed_levels,
            },
            app,
            state,
        )
        .await?;
    }
    Ok(ModelReasoningProbeResult {
        model_id: canonical,
        level,
        source_count: results.len(),
        available_count,
        applied_to_settings,
        sources: results,
    })
}

fn normalized_reasoning_probe_level(model: &str, level: String) -> LocalResult<String> {
    let mut normalized = zenith_relay_core::normalize_model_reasoning_allowed_levels(
        BTreeMap::from([(model.to_string(), vec![level])]),
    )
    .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    normalized
        .remove(&model.to_ascii_lowercase())
        .and_then(|levels| levels.into_iter().next())
        .ok_or_else(|| {
            LocalPoolError::new(ErrorCode::InvalidState, "reasoning probe level is invalid")
        })
}

fn source_probe_binding(
    source: &ProviderSourceRecord,
    model: &str,
) -> Option<(SourceProtocolBinding, String)> {
    source
        .effective_protocol_bindings()
        .ok()?
        .into_iter()
        .filter(|binding| binding.wire_api == WireApi::Responses)
        .find_map(|binding| {
            binding
                .model_ids
                .iter()
                .find(|candidate| candidate.eq_ignore_ascii_case(model))
                .cloned()
                .map(|source_model| (binding, source_model))
        })
}

async fn probe_source_reasoning(
    source: ProviderSourceRecord,
    api_key: String,
    binding: SourceProtocolBinding,
    source_model: String,
    level: String,
) -> ModelReasoningProbeSourceResult {
    let source_id = source.id.clone();
    let source_name = source.name.clone();
    let available = async {
        let connector = SourceConnector::new(
            &ProviderSource {
                id: source.id,
                name: source.name,
                base_url: source.base_url,
                api_key,
                wire_api: source.wire_api,
                models: source.models,
            },
            std::slice::from_ref(&binding),
        )
        .ok()?;
        let request = json!({
            "model": source_model.clone(),
            "input": "Reply with OK.",
            "max_output_tokens": 1,
            "reasoning": { "effort": level },
        });
        let prepared = binding
            .adapter
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Responses,
                request: &request,
                model: &source_model,
                stream: false,
                reasoning_mode: binding.reasoning_mode,
                cache_write_ttl: binding.cache_write_ttl,
                previous: None,
                response_scope: "reasoning-probe",
                response_id_seed: "reasoning-probe",
            })
            .ok()?;
        let url = connector.endpoint(binding.key(), &source_model, false)?;
        let (authorization_name, authorization) = connector.authorization_for_binding(&binding);
        let mut headers = connector.protocol_headers_for_binding(&binding);
        headers.insert(authorization_name, authorization);
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .ok()?;
        client
            .post(url)
            .headers(headers)
            .json(prepared.upstream_body())
            .send()
            .await
            .ok()
            .is_some_and(|response| response.status().is_success())
            .then_some(())
    }
    .await
    .is_some();
    ModelReasoningProbeSourceResult {
        source_id,
        source_name,
        available,
    }
}

fn canonical_pool_model(state: &DesktopState, model_id: &str) -> LocalResult<String> {
    let requested = model_id.trim();
    if !is_valid_model_id(requested) {
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

/// Returns the configured members of the managed Responses pool. Runtime
/// availability is intentionally applied later from the live scheduler.
pub(super) fn local_pool_member_ids(
    sources: &[ProviderSourceRecord],
    accounts: &[LocalAccountRecord],
) -> LocalResult<(BTreeSet<String>, BTreeSet<String>)> {
    let mut source_ids = BTreeSet::new();
    for source in sources.iter().filter(|source| source.in_pool) {
        if source
            .supports_wire_api(WireApi::Responses)
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?
        {
            source_ids.insert(source.id.clone());
        }
    }
    let account_ids = accounts
        .iter()
        .filter(|account| account.account.in_pool)
        .map(|account| account.account.id.clone())
        .collect();
    Ok((source_ids, account_ids))
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
            let supports_responses = source
                .supports_wire_api(WireApi::Responses)
                .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
            if !supports_responses {
                return Err(LocalPoolError::new(
                    ErrorCode::Conflict,
                    "only sources with a Responses-compatible route can join the local pool",
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
            let _ = super::profiles::refresh_active_codex_catalog(&state).await;
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

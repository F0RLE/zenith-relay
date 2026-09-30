use super::super::{
    connections::validate_source_record, fence_runtime_candidates, restart_or_rollback,
};
use crate::{
    files::atomic_write,
    local_pool::{
        accounts::{
            credentials::CredentialStore, proxy::COMMON_PROXY_SECRET_REF, NativeSecretBackend,
        },
        error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
        models::ProviderSourceRecord,
        state::DesktopState,
        store::secret_store,
    },
};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use zenith_relay_core::protocol::{
    AccountPresetRule, ConfigurationPreset, ConfigurationPresetApplyInput,
    ConfigurationPresetApplyResult, ConfigurationPresetPreview, ConfigurationPresetSettings,
    PresetQuotaPolicy, PresetRoutingPolicy, SourcePresetRule, CONFIGURATION_PRESET_FORMAT,
    CONFIGURATION_PRESET_SCHEMA_VERSION,
};

type CommandResult<T> = std::result::Result<T, CommandError>;

pub fn export_local_configuration_preset(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<Option<String>> {
    let preset = local_configuration_preset(&state)?;
    write_configuration_preset(&preset, &app)
}

pub(super) fn local_configuration_preset(
    state: &DesktopState,
) -> CommandResult<ConfigurationPreset> {
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
            legacy_protocol_mode: None,
            id: record.id,
            name: record.name,
            base_url: record.base_url.trim_end_matches('/').to_string(),
            pricing_provider: record.pricing_provider,
            official_provider_family: record.official_provider_family,
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
    let mut preset = ConfigurationPreset {
        format: CONFIGURATION_PRESET_FORMAT.to_string(),
        schema_version: CONFIGURATION_PRESET_SCHEMA_VERSION,
        settings: ConfigurationPresetSettings {
            sources,
            accounts,
            routing: PresetRoutingPolicy {
                tool_policy: Some(gateway.tool_policy),
                basis_points_enabled: gateway.basis_points_enabled,
                max_retry_candidates: gateway.max_retry_candidates,
                pool_routing: gateway.pool_routing,
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
    preset.settings.routing.pool_routing = Some(preset.settings.resolved_pool_routing());
    Ok(preset)
}

mod resolve;

use resolve::{
    local_configuration_diff, local_preset_revision, prepare_local_configuration_preset,
};

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
    let content = std::fs::read(&path).map_err(|_| {
        LocalPoolError::new(ErrorCode::Io, "configuration preset could not be read")
    })?;
    if content.len() > 1024 * 1024 {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "configuration preset is too large",
        )
        .into());
    }
    let preset: ConfigurationPreset = serde_json::from_slice(&content).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "configuration preset is invalid or contains unsupported fields",
        )
    })?;
    let prepared = prepare_local_configuration_preset(&state, preset)?;
    let base_revision = local_preset_revision(&prepared.current)?;
    let changes = local_configuration_diff(&prepared.current, &prepared.target)?;
    Ok(Some(ConfigurationPresetPreview {
        base_revision,
        preset: prepared.resolved,
        changes,
    }))
}

pub async fn apply_local_configuration_preset(
    input: ConfigurationPresetApplyInput,
    state: State<'_, DesktopState>,
) -> CommandResult<ConfigurationPresetApplyResult> {
    let _mutation = state.setup_guard().await;
    let prepared = prepare_local_configuration_preset(&state, input.preset)?;
    let current_revision = local_preset_revision(&prepared.current)?;
    if input.base_revision != current_revision {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "local configuration changed; preview the preset again",
        )
        .into());
    }
    let changes = local_configuration_diff(&prepared.current, &prepared.target)?;
    if changes.is_empty() {
        return Ok(ConfigurationPresetApplyResult {
            previous_revision: current_revision.clone(),
            revision: current_revision,
            changes,
        });
    }
    let (old_gateway, old_sources, old_accounts, old_keys) = {
        let store = state.store()?;
        (
            store.gateway().clone(),
            store.sources().to_vec(),
            store.accounts().to_vec(),
            store.keys().to_vec(),
        )
    };
    let source_rules = prepared
        .target
        .settings
        .sources
        .iter()
        .map(|rule| (rule.id.as_str(), rule))
        .collect::<std::collections::BTreeMap<_, _>>();
    let account_rules = prepared
        .target
        .settings
        .accounts
        .iter()
        .map(|rule| (rule.id.as_str(), rule))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut sources = old_sources.clone();
    for source in &mut sources {
        let rule = source_rules[source.id.as_str()];
        apply_source_preset_policy(source, rule);
        validate_source_record(&state, source)?;
    }
    let mut accounts = old_accounts.clone();
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    for account in &mut accounts {
        let rule = account_rules[account.account.id.as_str()];
        let credential = credentials.load(&account.account.id).map_err(|error| {
            LocalPoolError::new(ErrorCode::SecretStoreUnavailable, error.to_string())
        })?;
        let current_proxy_id = credential.as_ref().and_then(|credential| {
            credential
                .proxy_url()
                .and_then(|value| zenith_relay_core::proxy_reference_id(value).ok())
        });
        let current_bypass = credential
            .as_ref()
            .is_some_and(|credential| credential.bypass_common_proxy());
        if rule.proxy_id != current_proxy_id || rule.bypass_common_proxy != current_bypass {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "configuration preset references a different account proxy",
            )
            .into());
        }
        account.account.enabled = rule.enabled;
        account.account.in_pool = rule.in_pool;
        account.allowed_models = rule.allowed_models.clone();
        account.excluded_models = rule.excluded_models.clone();
        account.priority = rule.priority;
        account.weight = rule.weight.max(1);
    }
    let settings = &prepared.target.settings;
    let current_proxy_id = if old_gateway.common_proxy_configured {
        secret_store::load(COMMON_PROXY_SECRET_REF)?
            .as_deref()
            .and_then(|value| zenith_relay_core::proxy_reference_id(value).ok())
    } else {
        None
    };
    if settings.quota.common_proxy_id != current_proxy_id {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "configuration preset references a different common proxy",
        )
        .into());
    }
    let mut gateway = old_gateway.clone();
    gateway.max_retry_candidates = settings.routing.max_retry_candidates;
    if let Some(policy) = &settings.routing.tool_policy {
        gateway.tool_policy = policy.clone();
    }
    gateway.basis_points_enabled = settings.routing.basis_points_enabled;
    gateway.pool_routing = settings.routing.pool_routing.clone();
    gateway.default_service_tier = settings.routing.default_service_tier;
    gateway.image_base_model = settings.routing.image_base_model.clone();
    gateway.quota_request_timeout_seconds = settings.quota.request_timeout_seconds;
    gateway.account_proxy_required = settings.quota.account_proxy_required;
    gateway.hidden_models = settings.hidden_models.clone();
    gateway.model_price_overrides = settings.model_price_overrides.clone();
    gateway.model_reasoning_allowed_levels = settings.model_reasoning_allowed_levels.clone();
    gateway.model_service_tier_overrides = settings.model_service_tier_overrides.clone();
    gateway.model_display_order = settings.model_display_order.clone();
    // Presets may change several scopes (including source routes and the
    // account-proxy requirement) in two durable writes. Fence the previous
    // physical graph until the replacement runtime or rollback is published.
    let account_ids = old_accounts
        .iter()
        .map(|account| account.account.id.clone())
        .collect::<Vec<_>>();
    let source_ids = old_sources
        .iter()
        .map(|source| source.id.clone())
        .collect::<Vec<_>>();
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences = fence_runtime_candidates(runtime.as_deref(), &account_ids, &source_ids);
    state
        .store()?
        .replace_pool_records(sources, accounts, old_keys.clone())?;
    if let Err(error) = state
        .store()
        .and_then(|mut store| store.replace_gateway(gateway))
    {
        if let Err(restore) = state.store().and_then(|mut store| {
            store.replace_pool_records(old_sources.clone(), old_accounts.clone(), old_keys.clone())
        }) {
            return Err(super::super::fail_closed(
                &state,
                format!("preset gateway save failed: {error}; failed to restore pool: {restore}"),
            )
            .await
            .into());
        }
        return Err(error.into());
    }
    if let Err(error) = restart_or_rollback(&state, || {
        state
            .store()?
            .replace_pool_records(old_sources, old_accounts, old_keys)?;
        state.store()?.replace_gateway(old_gateway)?;
        Ok(())
    })
    .await
    {
        return Err(error.into());
    }
    let revision = local_preset_revision(&prepared.target)?;
    Ok(ConfigurationPresetApplyResult {
        previous_revision: current_revision,
        revision,
        changes,
    })
}

pub(super) fn apply_source_preset_policy(
    source: &mut ProviderSourceRecord,
    rule: &SourcePresetRule,
) {
    source.pricing_provider = rule.pricing_provider.clone();
    source.official_provider_family = rule.official_provider_family.clone();
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

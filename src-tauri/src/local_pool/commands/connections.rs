use super::{
    apply_source_policies_if_running, apply_source_policy_if_running, core_error,
    refresh_active_codex_catalog_in_background, refresh_local_gateway_key_scope_if_running,
    restart_after_secret_change, sync_records_or_rollback,
};
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
    models::{LocalPoolSnapshot, ProviderSourceRecord},
    state::DesktopState,
    store::secret_store,
};
use chrono::Utc;
use serde::Deserialize;
use std::collections::BTreeMap;
use tauri::{AppHandle, State};
use uuid::Uuid;
use zenith_relay_core::{
    discover_source_models_and_protocol_bindings, fetch_source_provider_stats,
    source_points_to_gateway, ApiModelPriceOverride, ProviderSource, SourceProtocolBinding,
    SourceProviderStats, WireApi,
};
#[cfg(test)]
use zenith_relay_core::{MessagesReasoningMode, SourceAdapter};

type CommandResult<T> = std::result::Result<T, CommandError>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSourceInput {
    name: String,
    base_url: String,
    api_key: String,
    #[serde(default = "responses_wire_api")]
    wire_api: WireApi,
    #[serde(default)]
    protocol_bindings: Vec<SourceProtocolBinding>,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    draining: bool,
    #[serde(default)]
    allowed_models: Vec<String>,
    #[serde(default)]
    excluded_models: Vec<String>,
    #[serde(default)]
    priority: i32,
    #[serde(default = "default_weight")]
    weight: u32,
    #[serde(default)]
    recovery_delay_seconds: u64,
    #[serde(default)]
    model_price_overrides: BTreeMap<String, ApiModelPriceOverride>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSourceInput {
    source_id: String,
    name: String,
    base_url: String,
    wire_api: WireApi,
    #[serde(default)]
    protocol_bindings: Option<Vec<SourceProtocolBinding>>,
    models: Vec<String>,
    #[serde(default)]
    in_pool: Option<bool>,
    #[serde(default)]
    draining: bool,
    #[serde(default)]
    allowed_models: Vec<String>,
    #[serde(default)]
    excluded_models: Vec<String>,
    priority: i32,
    #[serde(default)]
    source_priorities: BTreeMap<String, i32>,
    weight: u32,
    #[serde(default)]
    recovery_delay_seconds: u64,
    #[serde(default)]
    model_price_overrides: Option<BTreeMap<String, ApiModelPriceOverride>>,
}

#[tauri::command]
pub async fn create_local_source(
    input: CreateSourceInput,
    state: State<'_, DesktopState>,
) -> CommandResult<ProviderSourceRecord> {
    let _mutation = state.setup_guard().await;
    let id = format!("source_{}", Uuid::new_v4().simple());
    let secret_ref = format!("source:{id}");
    let mut runtime_source = ProviderSource {
        id: id.clone(),
        name: input.name.trim().to_string(),
        base_url: input.base_url.trim().to_string(),
        api_key: input.api_key.trim().to_string(),
        wire_api: input.wire_api,
        models: input.models,
    };
    runtime_source.validate().map_err(core_error)?;
    ensure_not_gateway_self_source(&state, &runtime_source.base_url)?;
    let discovery =
        discover_source_models_and_protocol_bindings(&runtime_source, &input.protocol_bindings)
            .await
            .map_err(core_error)?;
    runtime_source.models = discovery.models;
    if runtime_source.models.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "source did not expose any configured models",
        )
        .into());
    }

    let mut record = ProviderSourceRecord {
        id,
        name: runtime_source.name,
        enabled: true,
        in_pool: false,
        draining: input.draining,
        base_url: runtime_source.base_url,
        secret_ref: secret_ref.clone(),
        wire_api: runtime_source.wire_api,
        protocol_bindings: discovery.protocol_bindings,
        models: runtime_source.models,
        allowed_models: input.allowed_models,
        excluded_models: input.excluded_models,
        priority: input.priority,
        weight: input.weight,
        recovery_delay_seconds: input.recovery_delay_seconds,
        model_price_overrides: input.model_price_overrides,
        last_used_at: None,
        last_test_at: Some(Utc::now().to_rfc3339()),
        last_test_status: Some("ok".into()),
        last_error: None,
    };
    record.normalize();
    record
        .normalize_protocol_bindings()
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error))?;
    let (old_sources, old_keys) = current_records(&state)?;
    secret_store::save(&secret_ref, &runtime_source.api_key)?;
    if let Err(error) = state.store()?.upsert_source(record.clone()) {
        cleanup_created_secret(&secret_ref, &error)?;
        return Err(error.into());
    }
    if let Err(error) = sync_records_or_rollback(&state, old_sources, old_keys).await {
        let source_was_rolled_back = state.store()?.source(&record.id).is_none();
        if source_was_rolled_back {
            cleanup_created_secret(&secret_ref, &error)?;
        }
        return Err(error.into());
    }
    Ok(record)
}

#[tauri::command]
pub async fn update_local_source(
    input: UpdateSourceInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let source_priorities = input.source_priorities.clone();
    let current = state
        .store()?
        .source(&input.source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    let mut updated = ProviderSourceRecord {
        id: current.id.clone(),
        name: input.name,
        enabled: current.enabled,
        in_pool: current.in_pool,
        draining: input.draining,
        base_url: input.base_url,
        secret_ref: current.secret_ref.clone(),
        wire_api: input.wire_api,
        protocol_bindings: input.protocol_bindings.unwrap_or(current.protocol_bindings),
        models: input.models,
        allowed_models: input.allowed_models,
        excluded_models: input.excluded_models,
        priority: input.priority,
        weight: input.weight,
        recovery_delay_seconds: input.recovery_delay_seconds,
        model_price_overrides: input
            .model_price_overrides
            .unwrap_or(current.model_price_overrides),
        last_used_at: current.last_used_at,
        last_test_at: current.last_test_at,
        last_test_status: current.last_test_status,
        last_error: current.last_error,
    };
    if let Some(in_pool) = input.in_pool {
        updated.in_pool = in_pool;
    }
    updated.normalize();
    updated
        .normalize_protocol_bindings()
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error))?;
    validate_source_record(&state, &updated)?;
    let (old_sources, old_keys) = current_records(&state)?;
    let mut next_sources = old_sources.clone();
    let target = next_sources
        .iter_mut()
        .find(|source| source.id == updated.id)
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    *target = updated;
    apply_source_priorities(&mut next_sources, &source_priorities)?;
    let catalog_changed = source_catalog_visibility_changed(&old_sources, &next_sources);
    state
        .store()?
        .replace_records(next_sources.clone(), old_keys.clone())?;
    let updated_in_place = if source_runtime_policy_compatible(&old_sources, &next_sources)
        && apply_source_policies_if_running(&state, &old_sources, &next_sources).await
    {
        // A scope refresh is part of the same hot update. If it cannot be
        // applied, fall back to the existing restart-or-rollback path rather
        // than leaving policy and key scope out of sync.
        refresh_local_gateway_key_scope_if_running(&state)
            .await
            .unwrap_or(false)
    } else {
        false
    };
    if !updated_in_place {
        sync_records_or_rollback(&state, old_sources, old_keys).await?;
    }
    let snapshot = state.snapshot().await?;
    drop(_mutation);
    if updated_in_place && catalog_changed {
        refresh_active_codex_catalog_in_background(app);
    }
    Ok(snapshot)
}

#[tauri::command]
pub async fn set_local_source_enabled(
    source_id: String,
    enabled: bool,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let mut source = state
        .store()?
        .source(&source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    if source.enabled == enabled {
        return state.snapshot().await.map_err(Into::into);
    }
    if enabled {
        validate_source_record(&state, &source)?;
    }
    let (old_sources, old_keys) = current_records(&state)?;
    source.enabled = enabled;
    let catalog_changed = source.in_pool;
    state.store()?.upsert_source(source.clone())?;
    let updated_in_place = if apply_source_policy_if_running(&state, &old_sources, &source).await {
        refresh_local_gateway_key_scope_if_running(&state)
            .await
            .unwrap_or(false)
    } else {
        false
    };
    if !updated_in_place {
        sync_records_or_rollback(&state, old_sources, old_keys).await?;
    }
    let snapshot = state.snapshot().await?;
    drop(_mutation);
    if updated_in_place && catalog_changed {
        refresh_active_codex_catalog_in_background(app);
    }
    Ok(snapshot)
}

#[tauri::command]
pub async fn delete_local_source(
    source_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let source = state
        .store()?
        .source(&source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    let old_secret = secret_store::load(&source.secret_ref)?;
    let (old_sources, old_keys) = current_records(&state)?;
    let sources = old_sources
        .iter()
        .filter(|candidate| candidate.id != source_id)
        .cloned()
        .collect::<Vec<_>>();
    state.store()?.replace_records(sources, old_keys.clone())?;
    sync_records_or_rollback(&state, old_sources.clone(), old_keys.clone()).await?;

    if let Err(cleanup) = secret_store::delete(&source.secret_ref) {
        if let Some(secret) = old_secret {
            secret_store::save(&source.secret_ref, &secret).map_err(|restore| {
                LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("{cleanup}; failed to restore source secret: {restore}"),
                )
            })?;
            let (deleted_sources, deleted_keys) = current_records(&state)?;
            let restore_records = { state.store()?.replace_records(old_sources, old_keys) };
            if let Err(restore) = restore_records {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("{cleanup}; failed to restore deleted source records: {restore}"),
                )
                .into());
            }
            if let Err(restore) =
                sync_records_or_rollback(&state, deleted_sources, deleted_keys).await
            {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("{cleanup}; failed to restore gateway after source cleanup: {restore}"),
                )
                .into());
            }
        }
        return Err(cleanup.into());
    }
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn rotate_local_source_key(
    source_id: String,
    api_key: String,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let source = state
        .store()?
        .source(&source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    let api_key = api_key.trim().to_string();
    ProviderSource {
        id: source.id.clone(),
        name: source.name.clone(),
        base_url: source.base_url.clone(),
        api_key: api_key.clone(),
        wire_api: source.wire_api,
        models: source.models.clone(),
    }
    .validate()
    .map_err(core_error)?;
    ensure_not_gateway_self_source(&state, &source.base_url)?;
    let old_secret = secret_store::load(&source.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
    secret_store::save(&source.secret_ref, &api_key)?;
    restart_after_secret_change(&state, &source.secret_ref, &old_secret).await?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn test_local_source(
    source_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<ProviderSourceRecord> {
    refresh_local_source_models(&state, &source_id).await
}

pub(crate) async fn refresh_local_source_models(
    state: &DesktopState,
    source_id: &str,
) -> CommandResult<ProviderSourceRecord> {
    let source = state
        .store()?
        .source(source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    let api_key = secret_store::load(&source.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
    let runtime_source = ProviderSource {
        id: source.id.clone(),
        name: source.name.clone(),
        base_url: source.base_url.clone(),
        api_key: api_key.clone(),
        wire_api: source.wire_api,
        models: source.models.clone(),
    };
    ensure_not_gateway_self_source(state, &runtime_source.base_url)?;
    let discovery =
        discover_source_models_and_protocol_bindings(&runtime_source, &source.protocol_bindings)
            .await
            .map_err(core_error);

    let _mutation = state.setup_guard().await;
    let current = state
        .store()?
        .source(source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    let current_api_key = secret_store::load(&current.secret_ref)?;
    if !source_probe_matches(&source, &current)
        || current_api_key.as_deref() != Some(api_key.as_str())
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source changed while its models were being refreshed",
        )
        .into());
    }

    let discovery = match discovery {
        Ok(discovery) if !discovery.models.is_empty() => discovery,
        Ok(_) => {
            let error = LocalPoolError::new(
                ErrorCode::InvalidState,
                "source did not expose any configured models",
            );
            persist_source_refresh_failure(state, current, &error)?;
            return Err(error.into());
        }
        Err(error) => {
            persist_source_refresh_failure(state, current, &error)?;
            return Err(error.into());
        }
    };
    let runtime_changed = current.models != discovery.models
        || current.protocol_bindings != discovery.protocol_bindings;
    let mut updated = current;
    updated.models = discovery.models;
    updated.protocol_bindings = discovery.protocol_bindings;
    updated.last_test_at = Some(Utc::now().to_rfc3339());
    updated.last_test_status = Some("ok".into());
    updated.last_error = None;
    updated.normalize();
    updated
        .normalize_protocol_bindings()
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error))?;
    let (old_sources, old_keys) = current_records(state)?;
    state.store()?.upsert_source(updated.clone())?;
    if runtime_changed {
        sync_records_or_rollback(state, old_sources, old_keys).await?;
    }
    Ok(updated)
}

fn persist_source_refresh_failure(
    state: &DesktopState,
    mut source: ProviderSourceRecord,
    error: &LocalPoolError,
) -> CommandResult<()> {
    source.last_test_at = Some(Utc::now().to_rfc3339());
    source.last_test_status = Some("error".into());
    source.last_error = Some(error.message.clone());
    state.store()?.upsert_source(source)?;
    Ok(())
}

fn source_probe_matches(before: &ProviderSourceRecord, current: &ProviderSourceRecord) -> bool {
    before.base_url == current.base_url
        && before.secret_ref == current.secret_ref
        && before.wire_api == current.wire_api
        && before.protocol_bindings == current.protocol_bindings
        && before.models == current.models
}

fn source_runtime_policy_compatible(
    previous: &[ProviderSourceRecord],
    next: &[ProviderSourceRecord],
) -> bool {
    previous.len() == next.len()
        && previous.iter().all(|source| {
            next.iter()
                .find(|candidate| candidate.id == source.id)
                .is_some_and(|candidate| source_probe_matches(source, candidate))
        })
}

fn source_catalog_visibility_changed(
    previous: &[ProviderSourceRecord],
    next: &[ProviderSourceRecord],
) -> bool {
    previous.iter().any(|source| {
        let Some(candidate) = next.iter().find(|candidate| candidate.id == source.id) else {
            return source.in_pool;
        };
        (source.in_pool || candidate.in_pool)
            && (source.in_pool != candidate.in_pool
                || source.enabled != candidate.enabled
                || source.draining != candidate.draining
                || source.allowed_models != candidate.allowed_models
                || source.excluded_models != candidate.excluded_models)
    })
}

#[tauri::command]
pub async fn get_local_source_stats(
    source_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<SourceProviderStats> {
    let source = state
        .store()?
        .source(&source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    let api_key = secret_store::load(&source.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
    fetch_source_provider_stats(&source.base_url, &api_key)
        .await
        .map_err(|message| LocalPoolError::new(ErrorCode::GatewayUnavailable, message).into())
}

fn validate_source_record(state: &DesktopState, source: &ProviderSourceRecord) -> LocalResult<()> {
    if source.recovery_delay_seconds > 24 * 60 * 60 {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "source recovery delay must not exceed 24 hours",
        ));
    }
    source
        .validate_price_overrides()
        .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
    source
        .validate_protocol_bindings()
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error))?;
    let api_key = secret_store::load(&source.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
    if source.models.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "source must expose at least one model",
        ));
    }
    let runtime_source = ProviderSource {
        id: source.id.clone(),
        name: source.name.clone(),
        base_url: source.base_url.clone(),
        api_key,
        wire_api: source.wire_api,
        models: source.models.clone(),
    };
    runtime_source.validate().map_err(core_error)?;
    ensure_not_gateway_self_source(state, &runtime_source.base_url)
}

fn ensure_not_gateway_self_source(state: &DesktopState, base_url: &str) -> LocalResult<()> {
    let gateway = state.store()?.gateway().clone();
    let gateway_base_url = format!("http://{}:{}/v1", gateway.client_host, gateway.port);
    if source_points_to_gateway(base_url, &gateway_base_url) {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source base URL must not point back to this Relay gateway",
        ));
    }
    Ok(())
}

fn current_records(
    state: &DesktopState,
) -> LocalResult<(
    Vec<ProviderSourceRecord>,
    Vec<crate::local_pool::models::LocalGatewayKeyRecord>,
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

fn responses_wire_api() -> WireApi {
    WireApi::Responses
}

fn default_weight() -> u32 {
    1
}

fn apply_source_priorities(
    sources: &mut [ProviderSourceRecord],
    priorities: &BTreeMap<String, i32>,
) -> LocalResult<()> {
    for (source_id, priority) in priorities {
        let source = sources
            .iter_mut()
            .find(|source| source.id == *source_id)
            .ok_or_else(|| {
                LocalPoolError::new(ErrorCode::InvalidState, "source priority target not found")
            })?;
        source.priority = *priority;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_record() -> ProviderSourceRecord {
        ProviderSourceRecord {
            id: "source".into(),
            name: "Provider".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            base_url: "https://provider.test/v1".into(),
            secret_ref: "source:test".into(),
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            models: vec!["model-a".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: BTreeMap::new(),
            last_used_at: None,
            last_test_at: None,
            last_test_status: None,
            last_error: None,
        }
    }

    #[test]
    fn messages_wire_api_is_accepted_at_the_desktop_boundary() {
        let mut source = source_record();
        source.wire_api = WireApi::Messages;
        source.protocol_bindings = vec![SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            model_ids: source.models.clone(),
        }];

        source.normalize();
        source.normalize_protocol_bindings().unwrap();
        assert!(source.validate_protocol_bindings().is_ok());
    }

    #[test]
    fn source_priorities_are_applied_as_one_order() {
        let first = source_record();
        let mut second = source_record();
        second.id = "source-2".into();
        let mut sources = vec![first, second];

        apply_source_priorities(
            &mut sources,
            &BTreeMap::from([("source".into(), 2), ("source-2".into(), 1)]),
        )
        .unwrap();

        assert_eq!(sources[0].priority, 2);
        assert_eq!(sources[1].priority, 1);
        assert!(
            apply_source_priorities(&mut sources, &BTreeMap::from([("missing".into(), 3)]),)
                .is_err()
        );
    }

    #[test]
    fn mixed_binding_normalization_keeps_the_legacy_protocol_default_stable() {
        let mut source = source_record();
        source.protocol_bindings = vec![
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                model_ids: vec!["claude-native".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                model_ids: vec!["gpt-native".into()],
            },
        ];
        source.models = vec!["claude-native".into(), "gpt-native".into()];

        source.normalize_protocol_bindings().unwrap();

        assert_eq!(source.wire_api, WireApi::Responses);
        assert!(source.supports_wire_api(WireApi::Responses).unwrap());
        assert!(source.supports_wire_api(WireApi::Messages).unwrap());
    }

    #[test]
    fn source_probe_rejects_configuration_changes_but_ignores_runtime_status() {
        let before = source_record();
        let mut current = before.clone();
        current.last_test_status = Some("ok".into());
        assert!(source_probe_matches(&before, &current));

        current.models.push("model-b".into());
        assert!(!source_probe_matches(&before, &current));
    }

    #[test]
    fn source_catalog_refreshes_for_pool_membership_changes() {
        let inside = source_record();
        let mut outside = inside.clone();
        outside.in_pool = false;

        assert!(source_catalog_visibility_changed(
            std::slice::from_ref(&inside),
            std::slice::from_ref(&outside)
        ));
        assert!(source_catalog_visibility_changed(
            std::slice::from_ref(&outside),
            std::slice::from_ref(&inside)
        ));
    }
}

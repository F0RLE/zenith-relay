use super::*;
use crate::local_pool::models::LocalGatewayKeyRecord;

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
    let manual_models = normalize_model_ids(&runtime_source.models);
    let mut protocol_config = SourceProtocolConfig::automatic(&runtime_source.base_url);
    let (discovery, last_test_status, last_error) = if manual_models.is_empty() {
        match discover_source_with_protocol_config(
            &runtime_source,
            &input.protocol_bindings,
            &protocol_config,
        )
        .await
        {
            // An authenticated, valid empty catalog is a successful source
            // state. It remains editable and simply contributes zero routes
            // until a later refresh exposes models.
            Ok(discovery) => (discovery, "ok", None),
            Err(error) => {
                let error = core_error(error);
                (
                    empty_source_discovery(&runtime_source, &input.protocol_bindings)?,
                    "error",
                    Some(error.message),
                )
            }
        }
    } else {
        // A manual catalog is an explicit operator assertion for providers
        // that do not expose GET /models. Protocol bindings still go through
        // the same validation as an automatically discovered catalog.
        let protocol_bindings = protocol_config
            .resolve(
                &runtime_source.base_url,
                &manual_models,
                &input.protocol_bindings,
                runtime_source.wire_api,
            )
            .map_err(LocalPoolError::invalid_state)?;
        (
            SourceDiscovery {
                models: manual_models,
                protocol_bindings,
                resolved_base_url: None,
                detected_model_prices: BTreeMap::new(),
                capabilities: Vec::new(),
            },
            "manual",
            None,
        )
    };
    let mut protocol_bindings = Vec::new();
    let mut detected_model_prices = BTreeMap::new();
    discovery.apply_catalog(
        &mut runtime_source.base_url,
        &mut runtime_source.models,
        &mut protocol_bindings,
        &mut protocol_config,
        &mut detected_model_prices,
    );
    let mut record = ProviderSourceRecord {
        id,
        name: runtime_source.name,
        enabled: true,
        in_pool: false,
        draining: input.draining,
        base_url: runtime_source.base_url,
        secret_ref: secret_ref.clone(),
        pricing_provider: normalize_pricing_identity(input.pricing_provider)?,
        official_provider_family: normalize_pricing_identity(input.official_provider_family)?,
        wire_api: runtime_source.wire_api,
        protocol_bindings,
        protocol_config,
        models: runtime_source.models,
        allowed_models: input.allowed_models,
        excluded_models: input.excluded_models,
        priority: input.priority,
        weight: input.weight,
        recovery_delay_seconds: input.recovery_delay_seconds,
        model_price_overrides: input.model_price_overrides,
        detected_model_prices,
        last_used_at: None,
        last_test_at: Some(Utc::now().to_rfc3339()),
        last_test_status: Some(last_test_status.into()),
        last_error,
    };
    record.normalize();
    record
        .validate_protocol_bindings()
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
    let detected_model_prices =
        detected_prices_for_upstream(&current, &input.base_url, &input.wire_api);
    let mut protocol_config = current.protocol_config.clone();
    if input.base_url.trim() != current.base_url {
        protocol_config.invalidate(&input.base_url);
    }
    let mut updated = ProviderSourceRecord {
        id: current.id.clone(),
        name: input.name,
        enabled: current.enabled,
        in_pool: current.in_pool,
        draining: input.draining,
        base_url: input.base_url,
        secret_ref: current.secret_ref.clone(),
        pricing_provider: normalize_pricing_identity(
            input
                .pricing_provider
                .or_else(|| current.pricing_provider.clone()),
        )?,
        official_provider_family: normalize_pricing_identity(
            input
                .official_provider_family
                .or_else(|| current.official_provider_family.clone()),
        )?,
        wire_api: input.wire_api,
        protocol_bindings: input.protocol_bindings.unwrap_or(current.protocol_bindings),
        protocol_config,
        models: input.models,
        allowed_models: input.allowed_models,
        excluded_models: input.excluded_models,
        priority: input.priority,
        weight: input.weight,
        recovery_delay_seconds: input.recovery_delay_seconds,
        model_price_overrides: input
            .model_price_overrides
            .unwrap_or(current.model_price_overrides),
        detected_model_prices,
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
        .validate_protocol_bindings()
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
    let changed_source_ids = old_sources
        .iter()
        .filter(|source| {
            next_sources
                .iter()
                .find(|candidate| candidate.id == source.id)
                .is_some_and(|candidate| source_dispatch_configuration_changed(source, candidate))
        })
        .map(|source| source.id.clone())
        .collect::<Vec<_>>();
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences = fence_runtime_candidates(runtime.as_deref(), &[], &changed_source_ids);
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
    finish_source_hot_update(
        &state,
        app,
        _mutation,
        updated_in_place,
        catalog_changed,
        old_sources,
        old_keys,
    )
    .await
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
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences =
        fence_runtime_candidates(runtime.as_deref(), &[], std::slice::from_ref(&source_id));
    state.store()?.upsert_source(source.clone())?;
    let updated_in_place = if apply_source_policy_if_running(&state, &old_sources, &source).await {
        refresh_local_gateway_key_scope_if_running(&state)
            .await
            .unwrap_or(false)
    } else {
        false
    };
    finish_source_hot_update(
        &state,
        app,
        _mutation,
        updated_in_place,
        catalog_changed,
        old_sources,
        old_keys,
    )
    .await
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
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences =
        fence_runtime_candidates(runtime.as_deref(), &[], std::slice::from_ref(&source_id));
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
    let mut invalidated = source.clone();
    invalidated.protocol_config.invalidate(&source.base_url);
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences =
        fence_runtime_candidates(runtime.as_deref(), &[], std::slice::from_ref(&source_id));
    state.store()?.invalidate_source_refresh(&source_id)?;
    secret_store::save(&source.secret_ref, &api_key)?;
    if let Err(error) = state.store()?.upsert_source(invalidated) {
        secret_store::save(&source.secret_ref, &old_secret)?;
        return Err(error.into());
    }
    super::super::runtime::restart_or_rollback(&state, || {
        secret_store::save(&source.secret_ref, &old_secret)?;
        state.store()?.upsert_source(source)
    })
    .await?;
    state.snapshot().await.map_err(Into::into)
}

async fn finish_source_hot_update(
    state: &DesktopState,
    app: AppHandle,
    mutation: tokio::sync::MutexGuard<'_, ()>,
    updated_in_place: bool,
    catalog_changed: bool,
    old_sources: Vec<ProviderSourceRecord>,
    old_keys: Vec<LocalGatewayKeyRecord>,
) -> CommandResult<LocalPoolSnapshot> {
    if !updated_in_place {
        sync_records_or_rollback(state, old_sources, old_keys).await?;
    }
    let snapshot = state.snapshot().await?;
    drop(mutation);
    if updated_in_place && catalog_changed {
        refresh_active_codex_catalog_in_background(app);
    }
    Ok(snapshot)
}

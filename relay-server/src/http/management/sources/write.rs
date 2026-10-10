use super::*;

pub async fn create_source(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SourceInput>,
) -> Result<(StatusCode, Json<SourceSummary>), ManagementError> {
    validate_secret(&input.api_key, "source API key")?;
    let api_key = input.api_key.clone();
    let source_id = format!("source_{}", uuid::Uuid::new_v4().simple());
    let secret_ref = format!("source:{source_id}");
    let mut new_source_record = source_record(source_id, secret_ref.clone(), input)?;
    ensure_not_server_self_source(&state, &new_source_record.base_url)?;
    match discover_models(&new_source_record, &api_key).await {
        Ok(discovery) => {
            discovery.apply_catalog(
                &mut new_source_record.base_url,
                &mut new_source_record.models,
                &mut new_source_record.protocol_bindings,
                &mut new_source_record.protocol_config,
                &mut new_source_record.detected_model_prices,
            );
        }
        Err(error) => {
            // A catalog response is capability evidence, not a prerequisite
            // for saving credentials. Keep the source editable and surface
            // the failed discovery in its diagnostic status instead.
            clear_source_catalog(&mut new_source_record);
            new_source_record.last_error_code = Some(error.code);
        }
    }
    normalize_record_protocol_bindings(&mut new_source_record)?;
    state
        .vault
        .save(&secret_ref, &api_key)
        .map_err(vault_error)?;
    if let Err(error) = state.store.save_source(&new_source_record) {
        let _ = state.vault.delete(&secret_ref);
        return Err(store_error(error));
    }
    state
        .rebuild_runtime_or_rollback(|| {
            state.store.delete_source(&new_source_record.id)?;
            state.vault.delete(&secret_ref)?;
            Ok(())
        })
        .await
        .map_err(runtime_error)?;
    Ok((
        StatusCode::CREATED,
        Json(source_summary(&state, &new_source_record)?),
    ))
}

pub async fn update_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<SourcePatch>,
) -> Result<Json<SourceSummary>, ManagementError> {
    let _configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let mut source_record = find_source(&state, &id)?;
    let previous_source_record = source_record.clone();
    let source_priorities = input.source_priorities.clone();
    let previous_secret = state
        .vault
        .load(&source_record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::SOURCE_SECRET_MISSING, "source secret missing")
        })?;
    if let Some(source_name) = input.name {
        source_record.name = clean_label(&source_name, "source name")?;
    }
    if let Some(base_url) = input.base_url {
        source_record.base_url = base_url.trim().to_string();
    }
    if let Some(pricing_provider) = input.pricing_provider {
        source_record.pricing_provider =
            normalize_pricing_identity(Some(pricing_provider), "pricing provider")?;
    }
    if let Some(provider_family) = input.official_provider_family {
        source_record.official_provider_family =
            normalize_pricing_identity(Some(provider_family), "official provider family")?;
    }
    if let Some(wire_api) = input.wire_api {
        source_record.wire_api = wire_api;
    }
    if let Some(protocol_bindings) = input.protocol_bindings {
        source_record.protocol_bindings = protocol_bindings;
    }
    if source_record.base_url != previous_source_record.base_url || input.api_key.is_some() {
        source_record
            .protocol_config
            .invalidate(&source_record.base_url);
    }
    if let Some(model_ids) = input.models {
        source_record.models = normalized_values(model_ids);
    }
    if let Some(allowed_model_ids) = input.allowed_models {
        source_record.allowed_models = normalized_values(allowed_model_ids);
    }
    if let Some(excluded_model_ids) = input.excluded_models {
        source_record.excluded_models = normalized_values(excluded_model_ids);
    }
    if let Some(enabled) = input.enabled {
        source_record.enabled = enabled;
    }
    if let Some(in_pool) = input.in_pool {
        source_record.in_pool = in_pool;
    }
    if let Some(draining) = input.draining {
        source_record.draining = draining;
    }
    if let Some(priority) = input.priority {
        source_record.priority = priority;
    }
    if let Some(priority) = source_priorities.get(&id) {
        source_record.priority = *priority;
    }
    if let Some(weight) = input.weight {
        source_record.weight = valid_weight(weight)?;
    }
    if let Some(recovery_delay_seconds) = input.recovery_delay_seconds {
        source_record.recovery_delay_seconds = valid_recovery_delay(recovery_delay_seconds)?;
    }
    if let Some(model_price_overrides) = input.model_price_overrides {
        source_record.model_price_overrides = normalize_source_prices(model_price_overrides)?;
    }
    normalize_record_protocol_bindings(&mut source_record)?;
    if source_record.in_pool
        && !source_record.supports_any_wire_api().map_err(|message| {
            ManagementError::validation(error_codes::SOURCE_PROTOCOL_INVALID, message)
        })?
    {
        return Err(ManagementError::new(
            StatusCode::CONFLICT,
            error_codes::SOURCE_POOL_PROTOCOL_UNSUPPORTED,
            "source must expose at least one verified API route before joining the pool",
            "pool",
            false,
        ));
    }
    validate_source_record(
        &source_record,
        input.api_key.as_deref().unwrap_or(&previous_secret),
    )?;
    ensure_not_server_self_source(&state, &source_record.base_url)?;
    let source_order = if source_priorities.is_empty() {
        None
    } else {
        let previous_sources = state.store.sources().map_err(store_error)?;
        let mut updated_sources = previous_sources.clone();
        let target = updated_sources
            .iter_mut()
            .find(|source| source.id == source_record.id)
            .ok_or_else(|| {
                ManagementError::validation(
                    error_codes::SOURCE_PRIORITY_TARGET_NOT_FOUND,
                    "source priority target not found",
                )
            })?;
        *target = source_record.clone();
        apply_source_priorities(&mut updated_sources, &source_priorities)?;
        Some((previous_sources, updated_sources))
    };
    let (previous_sources, updated_sources) = match &source_order {
        Some((previous_source_order, updated_source_order)) => (
            previous_source_order.as_slice(),
            updated_source_order.as_slice(),
        ),
        None => (
            std::slice::from_ref(&previous_source_record),
            std::slice::from_ref(&source_record),
        ),
    };
    let policy_only_update = input.api_key.is_none()
        && source_runtime_policy_compatible(previous_sources, updated_sources);
    let permission_changed = source_dispatch_permission_changed(
        &previous_source_record,
        &source_record,
        input.api_key.is_some(),
    );
    // A source can own several protocol routes. Block all of their pending
    // dispatches before changing its durable key, address or permissions.
    // Pure ordering and recovery-delay edits do not revoke pending leases.
    let _dispatch_fences = if permission_changed {
        state
            .runtime()
            .map_err(runtime_error)?
            .map(|runtime| runtime.fence_source_dispatch(&source_record.id))
    } else {
        None
    };
    if let Some(secret) = input.api_key.as_deref() {
        validate_secret(secret, "source API key")?;
        state
            .store
            .invalidate_source_refresh(&source_record.id)
            .map_err(store_error)?;
        state
            .vault
            .save(&source_record.secret_ref, secret)
            .map_err(vault_error)?;
    }
    let save_result = match &source_order {
        Some((_, sources)) => state.store.save_sources(sources),
        None => state.store.save_source(&source_record),
    };
    if let Err(error) = save_result {
        if let Err(restore) = state
            .vault
            .save(&source_record.secret_ref, &previous_secret)
        {
            let _ = state.replace_runtime(None);
            return Err(runtime_error(format!(
                "source save failed; previous credential could not be restored: {restore}"
            )));
        }
        return Err(store_error(error));
    }
    let restore = || {
        restore_source_update(
            &state,
            source_order.as_ref(),
            &previous_source_record,
            &previous_secret,
        )
    };
    let runtime_applied = if policy_only_update {
        match apply_source_policies_if_running(&state, previous_sources, updated_sources) {
            Ok(applied) => applied,
            Err(error) => {
                let recovery = build.rollback_and_rebuild(&state, restore).await;
                return match recovery {
                    Ok(()) => Err(runtime_error(error)),
                    Err(recovery) => Err(runtime_error(format!("{error}; {recovery}"))),
                };
            }
        }
    } else {
        false
    };
    if !runtime_applied {
        build
            .rebuild_or_rollback(&state, restore)
            .await
            .map_err(runtime_error)?;
    }
    Ok(Json(source_summary(&state, &source_record)?))
}

fn restore_source_update(
    state: &AppState,
    source_order: Option<&(Vec<SourceRecord>, Vec<SourceRecord>)>,
    previous_source_record: &SourceRecord,
    previous_secret: &str,
) -> Result<(), String> {
    match source_order {
        Some((sources, _)) => state.store.save_sources(sources)?,
        None => state.store.save_source(previous_source_record)?,
    }
    state
        .vault
        .save(&previous_source_record.secret_ref, previous_secret)
}

fn apply_source_policies_if_running(
    state: &AppState,
    previous_sources: &[SourceRecord],
    updated: &[SourceRecord],
) -> Result<bool, String> {
    let updates = policy::updates(previous_sources, updated);
    let Some(runtime) = state.runtime()? else {
        return Ok(!state.store.gateway_enabled()?);
    };
    if !updates.is_empty() && !runtime.update_source_policies(&updates) {
        return Ok(false);
    }
    state.refresh_internal_gateway_key_scopes(&runtime)
}

fn apply_source_priorities(
    sources: &mut [SourceRecord],
    priorities: &BTreeMap<String, i32>,
) -> Result<(), ManagementError> {
    zenith_relay_core::apply_source_priorities(
        sources,
        priorities,
        |source| source.id.as_str(),
        |source, priority| source.priority = priority,
    )
    .map_err(|_| {
        ManagementError::validation(
            error_codes::SOURCE_PRIORITY_TARGET_NOT_FOUND,
            "source priority target not found",
        )
    })
}

pub async fn delete_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    let _configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let source_record = find_source(&state, &id)?;
    let secret = state
        .vault
        .load(&source_record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::SOURCE_SECRET_MISSING, "source secret missing")
        })?;
    let _dispatch_fences = state
        .runtime()
        .map_err(runtime_error)?
        .map(|runtime| runtime.fence_source_dispatch(&id));
    state.store.delete_source(&id).map_err(store_error)?;
    if let Err(error) = state.vault.delete(&source_record.secret_ref) {
        // The vault write outcome may be uncertain. Restore the source record, but
        // do not keep serving the old executor until reconciliation.
        let restore = state.store.save_source(&source_record);
        let _ = state.replace_runtime(None);
        restore.map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
        return Err(vault_error(error));
    }
    build
        .rebuild_or_rollback(&state, || {
            state.vault.save(&source_record.secret_ref, &secret)?;
            state.store.save_source(&source_record)?;
            Ok(())
        })
        .await
        .map_err(runtime_error)?;
    Ok(StatusCode::NO_CONTENT)
}

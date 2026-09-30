use super::*;

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
    match discover_models(&record, &api_key).await {
        Ok(discovery) => {
            discovery.apply_catalog(
                &mut record.base_url,
                &mut record.models,
                &mut record.protocol_bindings,
                &mut record.protocol_config,
                &mut record.detected_model_prices,
            );
        }
        Err(error) => {
            // A catalog response is capability evidence, not a prerequisite
            // for saving credentials. Keep the source editable and surface
            // the failed discovery in its diagnostic status instead.
            clear_source_catalog(&mut record);
            record.last_error_code = Some(error.code);
        }
    }
    normalize_record_protocol_bindings(&mut record)?;
    state
        .vault
        .save(&secret_ref, &api_key)
        .map_err(vault_error)?;
    if let Err(error) = state.store.save_source(&record) {
        let _ = state.vault.delete(&secret_ref);
        return Err(store_error(error));
    }
    state
        .rebuild_runtime_or_rollback(|| {
            state.store.delete_source(&record.id)?;
            state.vault.delete(&secret_ref)?;
            Ok(())
        })
        .await
        .map_err(runtime_error)?;
    Ok((StatusCode::CREATED, Json(source_summary(&state, &record)?)))
}

pub async fn update_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<SourcePatch>,
) -> Result<Json<SourceSummary>, ManagementError> {
    let _configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let mut record = find_source(&state, &id)?;
    let old_record = record.clone();
    let source_priorities = input.source_priorities.clone();
    let old_secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::SOURCE_SECRET_MISSING, "source secret missing")
        })?;
    if let Some(value) = input.name {
        record.name = clean_label(&value, "source name")?;
    }
    if let Some(value) = input.base_url {
        record.base_url = value.trim().to_string();
    }
    if let Some(value) = input.pricing_provider {
        record.pricing_provider = normalize_pricing_identity(Some(value), "pricing provider")?;
    }
    if let Some(value) = input.official_provider_family {
        record.official_provider_family =
            normalize_pricing_identity(Some(value), "official provider family")?;
    }
    if let Some(value) = input.wire_api {
        record.wire_api = value;
    }
    if let Some(value) = input.protocol_bindings {
        record.protocol_bindings = value;
    }
    if record.base_url != old_record.base_url || input.api_key.is_some() {
        record.protocol_config.invalidate(&record.base_url);
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
    if let Some(value) = source_priorities.get(&id) {
        record.priority = *value;
    }
    if let Some(value) = input.weight {
        record.weight = valid_weight(value)?;
    }
    if let Some(value) = input.recovery_delay_seconds {
        record.recovery_delay_seconds = valid_recovery_delay(value)?;
    }
    if let Some(value) = input.model_price_overrides {
        record.model_price_overrides = normalize_source_prices(value)?;
    }
    normalize_record_protocol_bindings(&mut record)?;
    if record.in_pool
        && !record.supports_any_wire_api().map_err(|message| {
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
    validate_source_record(&record, input.api_key.as_deref().unwrap_or(&old_secret))?;
    ensure_not_server_self_source(&state, &record.base_url)?;
    let source_order = if source_priorities.is_empty() {
        None
    } else {
        let old_sources = state.store.sources().map_err(store_error)?;
        let mut next_sources = old_sources.clone();
        let target = next_sources
            .iter_mut()
            .find(|source| source.id == record.id)
            .ok_or_else(|| {
                ManagementError::validation(
                    error_codes::SOURCE_PRIORITY_TARGET_NOT_FOUND,
                    "source priority target not found",
                )
            })?;
        *target = record.clone();
        apply_source_priorities(&mut next_sources, &source_priorities)?;
        Some((old_sources, next_sources))
    };
    let (previous_sources, next_sources) = match &source_order {
        Some((previous, next)) => (previous.as_slice(), next.as_slice()),
        None => (
            std::slice::from_ref(&old_record),
            std::slice::from_ref(&record),
        ),
    };
    let policy_only_update =
        input.api_key.is_none() && source_runtime_policy_compatible(previous_sources, next_sources);
    let permission_changed =
        source_dispatch_permission_changed(&old_record, &record, input.api_key.is_some());
    // A source can own several protocol routes. Block all of their pending
    // dispatches before changing its durable key, address or permissions.
    // Pure ordering and recovery-delay edits do not revoke pending leases.
    let _dispatch_fences = if permission_changed {
        state
            .runtime()
            .map_err(runtime_error)?
            .map(|runtime| runtime.fence_source_dispatch(&record.id))
    } else {
        None
    };
    if let Some(secret) = input.api_key.as_deref() {
        validate_secret(secret, "source API key")?;
        state
            .store
            .invalidate_source_refresh(&record.id)
            .map_err(store_error)?;
        state
            .vault
            .save(&record.secret_ref, secret)
            .map_err(vault_error)?;
    }
    let save_result = match &source_order {
        Some((_, sources)) => state.store.save_sources(sources),
        None => state.store.save_source(&record),
    };
    if let Err(error) = save_result {
        if let Err(restore) = state.vault.save(&record.secret_ref, &old_secret) {
            let _ = state.replace_runtime(None);
            return Err(runtime_error(format!(
                "source save failed; previous credential could not be restored: {restore}"
            )));
        }
        return Err(store_error(error));
    }
    let restore =
        || || restore_source_update(&state, source_order.as_ref(), &old_record, &old_secret);
    let runtime_applied = if policy_only_update {
        match apply_source_policies_if_running(&state, previous_sources, next_sources) {
            Ok(applied) => applied,
            Err(error) => {
                let recovery = build.rollback_and_rebuild(&state, restore()).await;
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
            .rebuild_or_rollback(&state, restore())
            .await
            .map_err(runtime_error)?;
    }
    Ok(Json(source_summary(&state, &record)?))
}

fn restore_source_update(
    state: &AppState,
    source_order: Option<&(Vec<SourceRecord>, Vec<SourceRecord>)>,
    old_record: &SourceRecord,
    old_secret: &str,
) -> Result<(), String> {
    match source_order {
        Some((sources, _)) => state.store.save_sources(sources)?,
        None => state.store.save_source(old_record)?,
    }
    state.vault.save(&old_record.secret_ref, old_secret)
}

fn apply_source_policies_if_running(
    state: &AppState,
    previous: &[SourceRecord],
    next: &[SourceRecord],
) -> Result<bool, String> {
    let updates = policy::updates(previous, next);
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
    let record = find_source(&state, &id)?;
    let secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::SOURCE_SECRET_MISSING, "source secret missing")
        })?;
    let _dispatch_fences = state
        .runtime()
        .map_err(runtime_error)?
        .map(|runtime| runtime.fence_source_dispatch(&id));
    state.store.delete_source(&id).map_err(store_error)?;
    if let Err(error) = state.vault.delete(&record.secret_ref) {
        // The vault write outcome may be uncertain. Restore the record, but
        // do not keep serving the old executor until reconciliation.
        let restore = state.store.save_source(&record);
        let _ = state.replace_runtime(None);
        restore.map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
        return Err(vault_error(error));
    }
    build
        .rebuild_or_rollback(&state, || {
            state.vault.save(&record.secret_ref, &secret)?;
            state.store.save_source(&record)?;
            Ok(())
        })
        .await
        .map_err(runtime_error)?;
    Ok(StatusCode::NO_CONTENT)
}

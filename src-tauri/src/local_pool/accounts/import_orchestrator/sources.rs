use super::*;
use chrono::Utc;
use sha2::{Digest, Sha256};

pub(crate) async fn import_source_item(
    state: &DesktopState,
    item: ParsedImportItem,
    add_to_pool: bool,
    discover_models: bool,
    configured_models: &[String],
) -> ItemResult<ProviderSourceRecord> {
    let api_key = item
        .secrets()
        .api_key()
        .map(str::to_string)
        .ok_or_else(|| ImportItemError::new("api_key_missing", "source API key is missing"))?;
    let base_url = imported_source_base_url(&item)?;
    let existing = find_existing_source(state, &base_url, &api_key)?;
    let wire_api = imported_source_wire_api(&item, existing.as_ref())?;
    let source_id = existing
        .as_ref()
        .map(|source| source.id.clone())
        .unwrap_or_else(|| format!("source_{}", Uuid::new_v4().simple()));
    let secret_ref = existing
        .as_ref()
        .map(|source| source.secret_ref.clone())
        .unwrap_or_else(|| format!("source:{source_id}"));
    let requested_models = if configured_models.is_empty() {
        existing
            .as_ref()
            .map(|source| source.models.clone())
            .unwrap_or_default()
    } else {
        configured_models.to_vec()
    };
    let mut runtime_source = ProviderSource {
        id: source_id.clone(),
        name: existing
            .as_ref()
            .map(|source| source.name.clone())
            .unwrap_or_else(|| item.label.trim().to_string()),
        base_url: base_url.clone(),
        api_key: api_key.clone(),
        wire_api,
        models: requested_models,
    };
    runtime_source
        .validate()
        .map_err(|_| ImportItemError::new("source_invalid", "imported source is invalid"))?;
    let discover_models = discover_models || runtime_source.models.is_empty();
    let (detected_model_prices, protocol_bindings) = if discover_models {
        let discovery = discover_source_models_and_protocol_bindings(&runtime_source, &[])
            .await
            .map_err(|_| {
                ImportItemError::new(
                    "source_model_discovery_failed",
                    "source model discovery failed",
                )
            })?;
        runtime_source.models = discovery.models;
        if let Some(base_url) = discovery.resolved_base_url {
            runtime_source.base_url = base_url;
        }
        (discovery.detected_model_prices, discovery.protocol_bindings)
    } else if !runtime_source.models.is_empty() {
        (
            existing
                .as_ref()
                .map(|source| source.detected_model_prices.clone())
                .unwrap_or_default(),
            existing
                .as_ref()
                .map(|source| source.protocol_bindings.clone())
                .unwrap_or_default(),
        )
    } else {
        return Err(ImportItemError::new(
            "models_required",
            "models are required when discovery is disabled",
        ));
    };
    if runtime_source.models.is_empty() {
        return Err(ImportItemError::new(
            "models_empty",
            "source did not expose any configured models",
        ));
    }

    let mut record = imported_source_record(
        &item,
        runtime_source,
        secret_ref,
        existing.as_ref(),
        protocol_bindings,
        detected_model_prices,
        discover_models.then(|| Utc::now().to_rfc3339()),
    );
    record.in_pool |= add_to_pool;
    persist_imported_source(state, &record, &api_key, existing.as_ref()).await?;
    Ok(record)
}

pub(crate) fn imported_source_record(
    item: &ParsedImportItem,
    runtime_source: ProviderSource,
    secret_ref: String,
    existing: Option<&ProviderSourceRecord>,
    protocol_bindings: Vec<SourceProtocolBinding>,
    detected_model_prices: BTreeMap<String, ApiModelPriceOverride>,
    tested_at: Option<String>,
) -> ProviderSourceRecord {
    let tested = tested_at.is_some();
    let mut record = ProviderSourceRecord {
        id: runtime_source.id,
        name: runtime_source.name,
        enabled: existing.as_ref().is_none_or(|source| source.enabled),
        in_pool: existing.as_ref().is_some_and(|source| source.in_pool),
        draining: existing.as_ref().is_some_and(|source| source.draining),
        base_url: runtime_source.base_url,
        secret_ref,
        wire_api: runtime_source.wire_api,
        protocol_bindings,
        models: runtime_source.models,
        allowed_models: existing
            .as_ref()
            .map(|source| source.allowed_models.clone())
            .unwrap_or_default(),
        excluded_models: existing
            .as_ref()
            .map(|source| source.excluded_models.clone())
            .unwrap_or_default(),
        priority: existing
            .as_ref()
            .map(|source| source.priority)
            .or(item.priority)
            .unwrap_or_default(),
        weight: existing.as_ref().map_or(1, |source| source.weight),
        recovery_delay_seconds: existing
            .as_ref()
            .map_or(0, |source| source.recovery_delay_seconds),
        model_price_overrides: existing
            .as_ref()
            .map(|source| source.model_price_overrides.clone())
            .unwrap_or_default(),
        detected_model_prices,
        last_used_at: existing
            .as_ref()
            .and_then(|source| source.last_used_at.clone()),
        last_test_at: tested_at.or_else(|| {
            existing
                .as_ref()
                .and_then(|source| source.last_test_at.clone())
        }),
        last_test_status: tested.then(|| "ok".to_string()).or_else(|| {
            existing
                .as_ref()
                .and_then(|source| source.last_test_status.clone())
        }),
        last_error: if tested {
            None
        } else {
            existing
                .as_ref()
                .and_then(|source| source.last_error.clone())
        },
    };
    record.normalize();
    record
}

pub(crate) fn imported_source_base_url(item: &ParsedImportItem) -> ItemResult<String> {
    if item.base_url_supplied && item.base_url.is_none() {
        return Err(ImportItemError::new(
            "source_base_url_invalid",
            "source base URL is invalid",
        ));
    }
    canonical_source_base_url(
        item.base_url
            .as_deref()
            .unwrap_or(DEFAULT_OPENAI_SOURCE_URL),
    )
}

pub(crate) fn imported_source_wire_api(
    item: &ParsedImportItem,
    existing: Option<&ProviderSourceRecord>,
) -> ItemResult<WireApi> {
    if item.protocol_supplied && item.protocol.is_none() {
        return Err(ImportItemError::new(
            "source_protocol_invalid",
            "source protocol is invalid",
        ));
    }
    match item.protocol.as_deref() {
        Some("responses") => Ok(WireApi::Responses),
        Some("chat_completions") => Ok(WireApi::ChatCompletions),
        None => Ok(existing.map_or(WireApi::Responses, |source| source.wire_api)),
        _ => Err(ImportItemError::new(
            "source_protocol_invalid",
            "source protocol is invalid",
        )),
    }
}

pub(crate) fn canonical_source_base_url(value: &str) -> ItemResult<String> {
    let mut url = Url::parse(value.trim()).map_err(|_| {
        ImportItemError::new("source_base_url_invalid", "source base URL is invalid")
    })?;
    let normalized_path = url.path().trim_end_matches('/').to_string();
    url.set_path(if normalized_path.is_empty() {
        "/"
    } else {
        &normalized_path
    });
    Ok(url.to_string().trim_end_matches('/').to_string())
}

pub(crate) fn source_identity_key(base_url: &str, api_key: &str) -> ItemResult<String> {
    let base_url = canonical_source_base_url(base_url)?;
    let secret_hash = hex::encode(Sha256::digest(api_key.as_bytes()));
    Ok(hex::encode(Sha256::digest(
        format!("source\0{base_url}\0{secret_hash}").as_bytes(),
    )))
}

pub(crate) fn find_existing_source(
    state: &DesktopState,
    base_url: &str,
    api_key: &str,
) -> ItemResult<Option<ProviderSourceRecord>> {
    let target = source_identity_key(base_url, api_key)?;
    let sources = state
        .store()
        .map_err(|_| ImportItemError::new("source_store_failed", "source store is unavailable"))?
        .sources()
        .to_vec();
    let mut matching = Vec::new();
    for source in sources {
        let Some(secret) = secret_store::load(&source.secret_ref).map_err(|_| {
            ImportItemError::new(
                "source_secret_store_failed",
                "source secret store is unavailable",
            )
        })?
        else {
            continue;
        };
        if source_identity_key(&source.base_url, &secret)? == target {
            matching.push(source);
        }
    }
    match matching.len() {
        0 => Ok(None),
        1 => Ok(matching.pop()),
        _ => Err(ImportItemError::recovery(
            "multiple local sources have the same credential identity",
        )),
    }
}

pub(crate) async fn persist_imported_source(
    state: &DesktopState,
    record: &ProviderSourceRecord,
    api_key: &str,
    existing: Option<&ProviderSourceRecord>,
) -> ItemResult<()> {
    let (old_sources, old_keys) = current_source_records(state)?;
    let old_secret = existing
        .map(|source| {
            secret_store::load(&source.secret_ref).map_err(|_| {
                ImportItemError::new(
                    "source_secret_store_failed",
                    "source secret store is unavailable",
                )
            })
        })
        .transpose()?
        .flatten();
    secret_store::save(&record.secret_ref, api_key).map_err(|_| {
        ImportItemError::new(
            "source_secret_store_failed",
            "failed to save source credentials",
        )
    })?;
    if state
        .store()
        .map_err(|_| ImportItemError::new("source_store_failed", "source store is unavailable"))?
        .upsert_source(record.clone())
        .is_err()
    {
        restore_source_secret(&record.secret_ref, old_secret.as_deref())?;
        return Err(ImportItemError::new(
            "source_store_failed",
            "failed to save source record",
        ));
    }
    if sync_records_or_rollback(state, old_sources, old_keys)
        .await
        .is_err()
    {
        let store = state.store().map_err(|_| {
            ImportItemError::new("source_store_failed", "source store is unavailable")
        })?;
        let rolled_back = match existing {
            Some(previous) => store.source(&record.id) == Some(previous),
            None => store.source(&record.id).is_none(),
        };
        drop(store);
        if rolled_back {
            restore_source_secret(&record.secret_ref, old_secret.as_deref())?;
        }
        return Err(ImportItemError::new(
            "gateway_sync_failed",
            "failed to apply source to the local gateway",
        ));
    }
    Ok(())
}

pub(crate) fn current_source_records(
    state: &DesktopState,
) -> ItemResult<(Vec<ProviderSourceRecord>, Vec<LocalGatewayKeyRecord>)> {
    let store = state
        .store()
        .map_err(|_| ImportItemError::new("source_store_failed", "source store is unavailable"))?;
    Ok((store.sources().to_vec(), store.keys().to_vec()))
}

pub(crate) fn restore_source_secret(secret_ref: &str, previous: Option<&str>) -> ItemResult<()> {
    match previous {
        Some(secret) => secret_store::save(secret_ref, secret),
        None => secret_store::delete(secret_ref),
    }
    .map_err(|_| ImportItemError::recovery("failed to restore previous source credentials"))
}

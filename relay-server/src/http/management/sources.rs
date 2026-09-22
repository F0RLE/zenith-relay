use super::{
    clean_label, default_weight, normalized_values, runtime_error, store_error, valid_weight,
    validate_secret, validation_error, vault_error, ManagementError,
};
use crate::state::{AppState, SourceRecord};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::SourceSummary;
use zenith_relay_core::{
    discover_source_with_protocol_config, fetch_source_provider_stats,
    normalize_model_price_overrides, source_points_to_gateway, ApiModelPriceOverride,
    ProviderSource, SourceDiscovery, SourceProtocolBinding, SourceProtocolConfig, WireApi,
};

mod policy;

use policy::source_runtime_policy_compatible;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sources", get(list_sources).post(create_source))
        .route("/sources/{id}", patch(update_source).delete(delete_source))
        .route("/sources/{id}/test", post(test_source))
        .route("/sources/{id}/probe", post(probe_source))
        .route("/sources/{id}/stats", get(source_stats))
}

pub async fn list_sources(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<SourceSummary>>, ManagementError> {
    Ok(Json(state.snapshot().map_err(store_error)?.sources))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInput {
    name: String,
    base_url: String,
    api_key: String,
    #[serde(default)]
    pricing_provider: Option<String>,
    #[serde(default)]
    official_provider_family: Option<String>,
    wire_api: WireApi,
    #[serde(default)]
    protocol_bindings: Vec<SourceProtocolBinding>,
    #[serde(default)]
    models: Vec<String>,
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
            if let Some(base_url) = discovery.resolved_base_url.as_deref() {
                record.base_url = base_url.to_string();
            }
            record.models = discovery.models;
            record.protocol_bindings = discovery.protocol_bindings;
            record.protocol_config.merge_catalog(discovery.capabilities);
            record.detected_model_prices = discovery.detected_model_prices;
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

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePatch {
    name: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
    #[serde(default)]
    pricing_provider: Option<String>,
    #[serde(default)]
    official_provider_family: Option<String>,
    wire_api: Option<WireApi>,
    protocol_bindings: Option<Vec<SourceProtocolBinding>>,
    models: Option<Vec<String>>,
    allowed_models: Option<Vec<String>>,
    excluded_models: Option<Vec<String>>,
    enabled: Option<bool>,
    in_pool: Option<bool>,
    draining: Option<bool>,
    priority: Option<i32>,
    #[serde(default)]
    source_priorities: BTreeMap<String, i32>,
    weight: Option<u32>,
    recovery_delay_seconds: Option<u64>,
    #[serde(default)]
    model_price_overrides: Option<BTreeMap<String, ApiModelPriceOverride>>,
}

pub async fn update_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<SourcePatch>,
) -> Result<Json<SourceSummary>, ManagementError> {
    let _configuration = state.configuration_lock.lock().await;
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
    if let Some(secret) = input.api_key.as_deref() {
        validate_secret(secret, "source API key")?;
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
        let _ = state.vault.save(&record.secret_ref, &old_secret);
        return Err(store_error(error));
    }
    let restore =
        || || restore_source_update(&state, source_order.as_ref(), &old_record, &old_secret);
    let runtime_applied = if policy_only_update {
        match apply_source_policies_if_running(&state, previous_sources, next_sources) {
            Ok(applied) => applied,
            Err(error) => {
                let recovery = state.rollback_and_rebuild_runtime(restore()).await;
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
        state
            .rebuild_runtime_or_rollback(restore())
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
    for (source_id, priority) in priorities {
        let source = sources
            .iter_mut()
            .find(|source| source.id == *source_id)
            .ok_or_else(|| {
                ManagementError::validation(
                    error_codes::SOURCE_PRIORITY_TARGET_NOT_FOUND,
                    "source priority target not found",
                )
            })?;
        source.priority = *priority;
    }
    Ok(())
}

pub async fn delete_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    let _configuration = state.configuration_lock.lock().await;
    let record = find_source(&state, &id)?;
    let secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::SOURCE_SECRET_MISSING, "source secret missing")
        })?;
    state.store.delete_source(&id).map_err(store_error)?;
    state
        .vault
        .delete(&record.secret_ref)
        .map_err(vault_error)?;
    state
        .rebuild_runtime_or_rollback(|| {
            state.vault.save(&record.secret_ref, &secret)?;
            state.store.save_source(&record)?;
            Ok(())
        })
        .await
        .map_err(runtime_error)?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn test_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SourceSummary>, ManagementError> {
    let record = find_source(&state, &id)?;
    let checked = record.clone();
    let api_key = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::SOURCE_SECRET_MISSING, "source secret missing")
        })?;
    ensure_not_server_self_source(&state, &record.base_url)?;
    let discovery = discover_models(&record, &api_key).await;
    let _configuration = state.configuration_lock.lock().await;
    let current = find_source(&state, &id)?;
    if current.protocol_config != checked.protocol_config
        || current.base_url != checked.base_url
        || current.wire_api != checked.wire_api
        || current.protocol_bindings != checked.protocol_bindings
        || current.models != checked.models
        || state
            .vault
            .load(&current.secret_ref)
            .map_err(vault_error)?
            .as_deref()
            != Some(api_key.as_str())
    {
        return Err(stale_probe_error());
    }
    let previous = current.clone();
    let mut record = current;
    let discovery = match discovery {
        Ok(discovery) => discovery,
        Err(error) => {
            // Even a failed refresh must not overwrite a concurrent edit.
            record.last_error_code = Some(error.code);
            state.store.save_source(&record).map_err(store_error)?;
            return Ok(Json(source_summary(&state, &record)?));
        }
    };
    if let Some(base_url) = discovery.resolved_base_url.as_deref() {
        record.base_url = base_url.to_string();
    }
    record.models = discovery.models;
    record.protocol_bindings = discovery.protocol_bindings;
    record.protocol_config.merge_catalog(discovery.capabilities);
    record.detected_model_prices = discovery.detected_model_prices;
    normalize_record_protocol_bindings(&mut record)?;
    record.last_error_code = None;
    state.store.save_source(&record).map_err(store_error)?;
    state
        .rebuild_runtime_or_rollback(|| {
            state.store.save_source(&previous)?;
            Ok(())
        })
        .await
        .map_err(runtime_error)?;
    Ok(Json(source_summary(&state, &record)?))
}

fn stale_probe_error() -> ManagementError {
    ManagementError::new(
        StatusCode::CONFLICT,
        error_codes::SOURCE_PROBE_STALE,
        "source changed during the check; refresh and try again",
        "source",
        false,
    )
}

pub async fn probe_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<zenith_relay_core::SourceProbeInput>,
) -> Result<Json<zenith_relay_core::SourceProbeResult>, ManagementError> {
    let record = find_source(&state, &id)?;
    if record.protocol_config.revision != input.expected_revision {
        return Err(stale_probe_error());
    }
    ensure_not_server_self_source(&state, &record.base_url)?;
    let api_key = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::SOURCE_SECRET_MISSING, "source secret missing")
        })?;
    let source = ProviderSource {
        id: record.id.clone(),
        name: record.name.clone(),
        base_url: record.base_url.clone(),
        api_key: api_key.clone(),
        wire_api: record.wire_api,
        models: record.models.clone(),
    };
    let result = zenith_relay_core::probe_source_generation(&source, &input)
        .await
        .map_err(source_discovery_error)?;
    let _configuration = state.configuration_lock.lock().await;
    let mut current = find_source(&state, &id)?;
    let previous = current.clone();
    if current.base_url != record.base_url
        || current.models != record.models
        || current.protocol_config != record.protocol_config
        || state
            .vault
            .load(&current.secret_ref)
            .map_err(vault_error)?
            .as_deref()
            != Some(api_key.as_str())
        || !current
            .protocol_config
            .apply_probe(input.expected_revision, result.capability.clone())
    {
        return Err(stale_probe_error());
    }
    normalize_record_protocol_bindings(&mut current)?;
    state.store.save_source(&current).map_err(store_error)?;
    state
        .rebuild_runtime_or_rollback(|| state.store.save_source(&previous))
        .await
        .map_err(runtime_error)?;
    Ok(Json(result))
}

pub async fn source_stats(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<zenith_relay_core::SourceProviderStats>, ManagementError> {
    let record = find_source(&state, &id)?;
    let api_key = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found(
                error_codes::SOURCE_SECRET_MISSING,
                "source secret is missing",
            )
        })?;
    ensure_not_server_self_source(&state, &record.base_url)?;
    fetch_source_provider_stats(&record.base_url, &api_key)
        .await
        .map(Json)
        .map_err(|_| {
            ManagementError::new(
                StatusCode::BAD_GATEWAY,
                error_codes::SOURCE_STATS_UNAVAILABLE,
                "source stats are unavailable",
                "source",
                true,
            )
        })
}

fn source_record(
    id: String,
    secret_ref: String,
    input: SourceInput,
) -> Result<SourceRecord, ManagementError> {
    let protocol_config = SourceProtocolConfig::automatic(&input.base_url);
    let mut record = SourceRecord {
        id,
        name: clean_label(&input.name, "source name")?,
        enabled: true,
        in_pool: false,
        draining: false,
        base_url: input.base_url.trim().to_string(),
        secret_ref,
        pricing_provider: normalize_pricing_identity(input.pricing_provider, "pricing provider")?,
        official_provider_family: normalize_pricing_identity(
            input.official_provider_family,
            "official provider family",
        )?,
        wire_api: input.wire_api,
        protocol_bindings: input.protocol_bindings,
        protocol_config,
        models: normalized_values(input.models),
        allowed_models: normalized_values(input.allowed_models),
        excluded_models: normalized_values(input.excluded_models),
        priority: input.priority,
        weight: valid_weight(input.weight)?,
        recovery_delay_seconds: valid_recovery_delay(input.recovery_delay_seconds)?,
        model_price_overrides: normalize_source_prices(input.model_price_overrides)?,
        detected_model_prices: BTreeMap::new(),
        last_error_code: None,
    };
    normalize_record_protocol_bindings(&mut record)?;
    validate_source_record(&record, &input.api_key)?;
    Ok(record)
}

fn validate_source_record(record: &SourceRecord, api_key: &str) -> Result<(), ManagementError> {
    normalize_pricing_identity(record.pricing_provider.clone(), "pricing provider")?;
    normalize_pricing_identity(
        record.official_provider_family.clone(),
        "official provider family",
    )?;
    valid_recovery_delay(record.recovery_delay_seconds)?;
    normalize_source_prices(record.model_price_overrides.clone())?;
    normalize_source_prices(record.detected_model_prices.clone())?;
    validate_record_protocol_bindings(record)?;
    ProviderSource {
        id: record.id.clone(),
        name: record.name.clone(),
        base_url: record.base_url.clone(),
        api_key: api_key.to_string(),
        wire_api: record.wire_api,
        models: record.models.clone(),
    }
    .validate()
    .map_err(|error| validation_error(error.to_string()))
}

fn normalize_pricing_identity(
    value: Option<String>,
    label: &str,
) -> Result<Option<String>, ManagementError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(ManagementError::validation(
            error_codes::SOURCE_PRICING_IDENTITY_INVALID,
            format!("{label} contains unsupported characters"),
        ));
    }
    Ok(Some(value))
}

fn validate_record_protocol_bindings(record: &SourceRecord) -> Result<(), ManagementError> {
    if record.protocol_bindings.is_empty() {
        return Ok(());
    }
    record
        .effective_protocol_bindings()
        .map(drop)
        .map_err(|error| validation_error(error.to_string()))
}

fn normalize_record_protocol_bindings(record: &mut SourceRecord) -> Result<(), ManagementError> {
    record
        .effective_protocol_bindings()
        .map(drop)
        .map_err(validation_error)
}

fn clear_source_binding_models(bindings: &mut [SourceProtocolBinding]) {
    for binding in bindings {
        binding.model_ids.clear();
    }
}

fn clear_source_catalog(record: &mut SourceRecord) {
    record.models.clear();
    record.detected_model_prices.clear();
    clear_source_binding_models(&mut record.protocol_bindings);
}

fn valid_recovery_delay(value: u64) -> Result<u64, ManagementError> {
    (value <= 24 * 60 * 60).then_some(value).ok_or_else(|| {
        ManagementError::validation(
            error_codes::SOURCE_RECOVERY_DELAY_INVALID,
            "source recovery delay must not exceed 24 hours",
        )
    })
}

fn normalize_source_prices(
    prices: BTreeMap<String, ApiModelPriceOverride>,
) -> Result<BTreeMap<String, ApiModelPriceOverride>, ManagementError> {
    normalize_model_price_overrides(prices).map_err(|message| {
        ManagementError::validation(error_codes::SOURCE_MODEL_PRICE_INVALID, message)
    })
}

async fn discover_models(
    record: &SourceRecord,
    api_key: &str,
) -> Result<SourceDiscovery, ManagementError> {
    let source = ProviderSource {
        id: record.id.clone(),
        name: record.name.clone(),
        base_url: record.base_url.clone(),
        api_key: api_key.to_string(),
        wire_api: record.wire_api,
        models: record.models.clone(),
    };
    let discovery = discover_source_with_protocol_config(
        &source,
        &record.protocol_bindings,
        &record.protocol_config,
    )
    .await
    .map_err(source_discovery_error)?;
    Ok(discovery)
}

fn source_discovery_error(error: zenith_relay_core::Error) -> ManagementError {
    let (status, retryable) = match &error {
        zenith_relay_core::Error::Validation(_) | zenith_relay_core::Error::UnsupportedWireApi => {
            (StatusCode::BAD_REQUEST, false)
        }
        zenith_relay_core::Error::UpstreamStatus(status) => (
            StatusCode::BAD_GATEWAY,
            *status == 408 || *status == 429 || *status >= 500,
        ),
        zenith_relay_core::Error::Upstream(_)
        | zenith_relay_core::Error::UpstreamBodyTooLarge
        | zenith_relay_core::Error::InvalidUpstreamResponse(_) => (StatusCode::BAD_GATEWAY, true),
    };
    ManagementError::new(
        status,
        error_codes::SOURCE_TEST_FAILED,
        error.to_string(),
        "upstream",
        retryable,
    )
}

fn find_source(state: &AppState, id: &str) -> Result<SourceRecord, ManagementError> {
    state
        .store
        .sources()
        .map_err(store_error)?
        .into_iter()
        .find(|record| record.id == id)
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::SOURCE_NOT_FOUND, "source not found")
        })
}

fn source_summary(
    state: &AppState,
    record: &SourceRecord,
) -> Result<SourceSummary, ManagementError> {
    state
        .snapshot()
        .map_err(store_error)?
        .sources
        .into_iter()
        .find(|value| value.id == record.id)
        .ok_or_else(|| {
            ManagementError::internal(error_codes::SNAPSHOT_MISSING, "source snapshot missing")
        })
}

fn ensure_not_server_self_source(
    state: &AppState,
    source_base_url: &str,
) -> Result<(), ManagementError> {
    let gateway_base_url = format!(
        "{}/v1",
        state.config.public_base_url.as_str().trim_end_matches('/')
    );
    if source_points_to_gateway(source_base_url, &gateway_base_url) {
        return Err(ManagementError::validation(
            error_codes::SOURCE_SELF_ROUTE,
            "source base URL must not point back to this Relay gateway",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::store::{Store, Vault};
    use axum::extract::{Path, State};
    use std::collections::BTreeMap;
    use tempfile::TempDir;
    use zenith_relay_core::Error;

    fn test_state(root: &TempDir, port: u16) -> Arc<AppState> {
        let config = Config::for_test(
            root.path().to_path_buf(),
            format!("127.0.0.1:{port}").parse().unwrap(),
        );
        let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
        let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
        AppState::new(config, store, vault).unwrap()
    }

    #[test]
    fn upstream_404_is_a_non_retryable_bad_gateway_error() {
        let error = source_discovery_error(Error::UpstreamStatus(404));

        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert_eq!(error.code, "source_test_failed");
        assert_eq!(error.stage, "upstream");
        assert!(!error.retryable);
        assert!(error.message.contains("404"));
    }

    #[test]
    fn upstream_server_failures_remain_retryable() {
        let error = source_discovery_error(Error::UpstreamStatus(503));

        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert!(error.retryable);
    }

    #[test]
    fn failed_refresh_clears_the_source_catalog() {
        let mut record = SourceRecord {
            id: "source".to_string(),
            name: "Provider".to_string(),
            enabled: true,
            in_pool: true,
            draining: false,
            base_url: "https://provider.test/v1".to_string(),
            secret_ref: "source:source".to_string(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_config: Default::default(),
            protocol_bindings: vec![SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: zenith_relay_core::SourceAdapter::Native,
                reasoning_mode: zenith_relay_core::MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["model-a".to_string()],
            }],
            models: vec!["model-a".to_string()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: BTreeMap::new(),
            detected_model_prices: BTreeMap::from([(
                "model-a".to_string(),
                ApiModelPriceOverride {
                    input_micro_usd_per_million: 1,
                    cached_input_micro_usd_per_million: None,
                    cache_write_5m_micro_usd_per_million: None,
                    cache_write_1h_micro_usd_per_million: None,
                    output_micro_usd_per_million: 1,
                },
            )]),
            last_error_code: None,
        };

        clear_source_catalog(&mut record);

        assert!(record.models.is_empty());
        assert!(record.detected_model_prices.is_empty());
        assert!(record.protocol_bindings[0].model_ids.is_empty());
    }

    #[tokio::test]
    async fn source_stats_rejects_a_self_route_before_contacting_gateway() {
        let root = TempDir::new().unwrap();
        let state = test_state(&root, 45_678);
        let record = SourceRecord {
            id: "self-source".to_string(),
            name: "Self source".to_string(),
            enabled: true,
            in_pool: false,
            draining: false,
            base_url: format!(
                "{}/v1",
                state.config.public_base_url.as_str().trim_end_matches('/')
            ),
            secret_ref: "source:self-source".to_string(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_config: Default::default(),
            protocol_bindings: Vec::new(),
            models: vec!["provider/model".to_string()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: BTreeMap::new(),
            detected_model_prices: BTreeMap::new(),
            last_error_code: None,
        };
        state.store.save_source(&record).unwrap();
        state
            .vault
            .save(&record.secret_ref, "source-secret")
            .unwrap();

        let error = source_stats(State(state), Path(record.id))
            .await
            .expect_err("a source pointing at this Relay must be rejected");

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.code, "source_self_route");
        assert_eq!(error.stage, "validation");
        assert!(!error.retryable);
    }
}

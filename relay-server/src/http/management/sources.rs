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
use zenith_relay_core::protocol::SourceSummary;
use zenith_relay_core::{
    discover_source_models_and_protocol_bindings, fetch_source_provider_stats,
    normalize_model_price_overrides, normalize_source_protocol_bindings, source_points_to_gateway,
    ApiModelPriceOverride, ProviderSource, RuntimeCandidatePolicy, RuntimeSourcePolicyUpdate,
    SourceDiscovery, SourceProtocolBinding, WireApi,
};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sources", get(list_sources).post(create_source))
        .route("/sources/{id}", patch(update_source).delete(delete_source))
        .route("/sources/{id}/test", post(test_source))
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
    let discovery = discover_models(&record, &api_key).await?;
    record.models = discovery.models;
    record.protocol_bindings = discovery.protocol_bindings;
    normalize_record_protocol_bindings(&mut record)?;
    state
        .vault
        .save(&secret_ref, &api_key)
        .map_err(vault_error)?;
    if let Err(error) = state.store.save_source(&record) {
        let _ = state.vault.delete(&secret_ref);
        return Err(store_error(error));
    }
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.store.delete_source(&record.id);
        let _ = state.vault.delete(&secret_ref);
        return Err(runtime_error(error));
    }
    Ok((StatusCode::CREATED, Json(source_summary(&state, &record)?)))
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourcePatch {
    name: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
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
    let mut record = find_source(&state, &id)?;
    let old_record = record.clone();
    let source_priorities = input.source_priorities.clone();
    let old_secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found("source_secret_missing", "source secret missing")
        })?;
    if let Some(value) = input.name {
        record.name = clean_label(&value, "source name")?;
    }
    if let Some(value) = input.base_url {
        record.base_url = value.trim().to_string();
    }
    if let Some(value) = input.wire_api {
        record.wire_api = value;
    }
    if let Some(value) = input.protocol_bindings {
        record.protocol_bindings = value;
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
        && !record
            .supports_wire_api(WireApi::Responses)
            .map_err(|message| ManagementError::validation("source_protocol_invalid", message))?
    {
        return Err(ManagementError::new(
            StatusCode::CONFLICT,
            "source_pool_protocol_unsupported",
            "only Responses API sources can join the ChatGPT pool",
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
                    "source_priority_target_not_found",
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
    let runtime_applied = if policy_only_update {
        match apply_source_policies_if_running(&state, previous_sources, next_sources) {
            Ok(applied) => applied,
            Err(error) => {
                match &source_order {
                    Some((sources, _)) => {
                        let _ = state.store.save_sources(sources);
                    }
                    None => {
                        let _ = state.store.save_source(&old_record);
                    }
                }
                let _ = state.vault.save(&old_record.secret_ref, &old_secret);
                let _ = state.rebuild_runtime().await;
                return Err(runtime_error(error));
            }
        }
    } else {
        false
    };
    if !runtime_applied {
        if let Err(error) = state.rebuild_runtime().await {
            match &source_order {
                Some((sources, _)) => {
                    let _ = state.store.save_sources(sources);
                }
                None => {
                    let _ = state.store.save_source(&old_record);
                }
            }
            let _ = state.vault.save(&old_record.secret_ref, &old_secret);
            let _ = state.rebuild_runtime().await;
            return Err(runtime_error(error));
        }
    }
    Ok(Json(source_summary(&state, &record)?))
}

fn apply_source_policies_if_running(
    state: &AppState,
    previous: &[SourceRecord],
    next: &[SourceRecord],
) -> Result<bool, String> {
    let updates = next
        .iter()
        .filter(|source| {
            previous
                .iter()
                .find(|previous| previous.id == source.id)
                .is_none_or(|previous| source_runtime_policy_changed(previous, source))
        })
        .map(|source| RuntimeSourcePolicyUpdate {
            source_id: source.id.clone(),
            policy: RuntimeCandidatePolicy {
                enabled: source.enabled,
                draining: source.draining,
                priority: source.priority,
                weight: source.weight,
                allowed_models: source.allowed_models.clone(),
                excluded_models: source.excluded_models.clone(),
            },
            recovery_delay_seconds: source.recovery_delay_seconds,
        })
        .collect::<Vec<_>>();
    let Some(runtime) = state.runtime()? else {
        return Ok(!state.store.gateway_enabled()?);
    };
    if !updates.is_empty() && !runtime.update_source_policies(&updates) {
        return Ok(false);
    }
    state.refresh_internal_gateway_key_scopes(&runtime)
}

fn source_runtime_policy_compatible(previous: &[SourceRecord], next: &[SourceRecord]) -> bool {
    previous.len() == next.len()
        && previous.iter().all(|source| {
            next.iter()
                .find(|candidate| candidate.id == source.id)
                .is_some_and(|candidate| {
                    source.base_url == candidate.base_url
                        && source.secret_ref == candidate.secret_ref
                        && source.wire_api == candidate.wire_api
                        && source.protocol_bindings == candidate.protocol_bindings
                        && source.models == candidate.models
                })
        })
}

fn source_runtime_policy_changed(previous: &SourceRecord, next: &SourceRecord) -> bool {
    previous.enabled != next.enabled
        || previous.draining != next.draining
        || previous.priority != next.priority
        || previous.weight != next.weight
        || previous.allowed_models != next.allowed_models
        || previous.excluded_models != next.excluded_models
        || previous.recovery_delay_seconds != next.recovery_delay_seconds
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
                    "source_priority_target_not_found",
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
    let record = find_source(&state, &id)?;
    let secret = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found("source_secret_missing", "source secret missing")
        })?;
    state.store.delete_source(&id).map_err(store_error)?;
    state
        .vault
        .delete(&record.secret_ref)
        .map_err(vault_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.vault.save(&record.secret_ref, &secret);
        let _ = state.store.save_source(&record);
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn test_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SourceSummary>, ManagementError> {
    let mut record = find_source(&state, &id)?;
    let previous = record.clone();
    let api_key = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found("source_secret_missing", "source secret missing")
        })?;
    ensure_not_server_self_source(&state, &record.base_url)?;
    let discovery = match discover_models(&record, &api_key).await {
        Ok(discovery) => discovery,
        Err(error) => {
            record.last_error_code = Some(error.code.clone());
            state.store.save_source(&record).map_err(store_error)?;
            return Err(error);
        }
    };
    record.models = discovery.models;
    record.protocol_bindings = discovery.protocol_bindings;
    normalize_record_protocol_bindings(&mut record)?;
    record.last_error_code = None;
    state.store.save_source(&record).map_err(store_error)?;
    if let Err(error) = state.rebuild_runtime().await {
        let _ = state.store.save_source(&previous);
        let _ = state.rebuild_runtime().await;
        return Err(runtime_error(error));
    }
    Ok(Json(source_summary(&state, &record)?))
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
            ManagementError::not_found("source_secret_missing", "source secret is missing")
        })?;
    fetch_source_provider_stats(&record.base_url, &api_key)
        .await
        .map(Json)
        .map_err(|_| {
            ManagementError::new(
                StatusCode::BAD_GATEWAY,
                "source_stats_unavailable",
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
    let mut record = SourceRecord {
        id,
        name: clean_label(&input.name, "source name")?,
        enabled: true,
        in_pool: false,
        draining: false,
        base_url: input.base_url.trim().to_string(),
        secret_ref,
        wire_api: input.wire_api,
        protocol_bindings: input.protocol_bindings,
        models: normalized_values(input.models),
        allowed_models: normalized_values(input.allowed_models),
        excluded_models: normalized_values(input.excluded_models),
        priority: input.priority,
        weight: valid_weight(input.weight)?,
        recovery_delay_seconds: valid_recovery_delay(input.recovery_delay_seconds)?,
        model_price_overrides: normalize_source_prices(input.model_price_overrides)?,
        last_error_code: None,
    };
    normalize_record_protocol_bindings(&mut record)?;
    validate_source_record(&record, &input.api_key)?;
    Ok(record)
}

fn validate_source_record(record: &SourceRecord, api_key: &str) -> Result<(), ManagementError> {
    valid_recovery_delay(record.recovery_delay_seconds)?;
    normalize_source_prices(record.model_price_overrides.clone())?;
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

fn validate_record_protocol_bindings(record: &SourceRecord) -> Result<(), ManagementError> {
    if record.protocol_bindings.is_empty() {
        return Ok(());
    }
    normalize_source_protocol_bindings(
        record.protocol_bindings.clone(),
        record.wire_api,
        &record.models,
    )
    .map(drop)
    .map_err(|error| validation_error(error.to_string()))
}

fn normalize_record_protocol_bindings(record: &mut SourceRecord) -> Result<(), ManagementError> {
    if record.protocol_bindings.is_empty() {
        return Ok(());
    }
    record.protocol_bindings = normalize_source_protocol_bindings(
        std::mem::take(&mut record.protocol_bindings),
        record.wire_api,
        &record.models,
    )
    .map_err(|error| validation_error(error.to_string()))?;
    Ok(())
}

fn valid_recovery_delay(value: u64) -> Result<u64, ManagementError> {
    (value <= 24 * 60 * 60).then_some(value).ok_or_else(|| {
        ManagementError::validation(
            "source_recovery_delay_invalid",
            "source recovery delay must not exceed 24 hours",
        )
    })
}

fn normalize_source_prices(
    prices: BTreeMap<String, ApiModelPriceOverride>,
) -> Result<BTreeMap<String, ApiModelPriceOverride>, ManagementError> {
    normalize_model_price_overrides(prices)
        .map_err(|message| ManagementError::validation("source_model_price_invalid", message))
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
    let discovery =
        discover_source_models_and_protocol_bindings(&source, &record.protocol_bindings)
            .await
            .map_err(|_| {
                ManagementError::new(
                    StatusCode::BAD_GATEWAY,
                    "source_test_failed",
                    "source model discovery failed",
                    "upstream",
                    true,
                )
            })?;
    if discovery.models.is_empty() {
        return Err(ManagementError::validation(
            "source_models_empty",
            "source did not expose any models",
        ));
    }
    Ok(discovery)
}

fn find_source(state: &AppState, id: &str) -> Result<SourceRecord, ManagementError> {
    state
        .store
        .sources()
        .map_err(store_error)?
        .into_iter()
        .find(|record| record.id == id)
        .ok_or_else(|| ManagementError::not_found("source_not_found", "source not found"))
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
        .ok_or_else(|| ManagementError::internal("snapshot_missing", "source snapshot missing"))
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
            "source_self_route",
            "source base URL must not point back to this Relay gateway",
        ));
    }
    Ok(())
}

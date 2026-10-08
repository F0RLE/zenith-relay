mod identity;
mod persist;
mod record;

#[cfg(test)]
pub(crate) use identity::source_identity_key;
pub(crate) use identity::{
    find_existing_source, imported_source_base_url, imported_source_wire_api,
};
pub(crate) use persist::persist_imported_source;
pub(crate) use record::imported_source_record;

use super::{ImportItemError, ItemResult};
use crate::local_pool::models::ProviderSourceRecord;
use crate::local_pool::state::DesktopState;
use chrono::Utc;
use std::collections::BTreeMap;
use uuid::Uuid;
use zenith_relay_core::accounts::ParsedImportItem;
use zenith_relay_core::error_codes;
use zenith_relay_core::{discover_source_models_and_protocol_bindings, ProviderSource};

pub(crate) async fn import_source_item(
    state: &DesktopState,
    import_item: ParsedImportItem,
    add_to_pool: bool,
    discover_models: bool,
    configured_models: &[String],
) -> ItemResult<ProviderSourceRecord> {
    crate::diagnostics::breadcrumb("source-import", "item_started", &[]);
    let api_key = import_item
        .secrets()
        .api_key()
        .map(str::to_string)
        .ok_or_else(|| {
            ImportItemError::new(error_codes::API_KEY_MISSING, "source API key is missing")
        })?;
    let base_url = imported_source_base_url(&import_item)?;
    let existing = find_existing_source(state, &base_url, &api_key)?;
    let wire_api = imported_source_wire_api(&import_item, existing.as_ref())?;
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
            .unwrap_or_else(|| import_item.label.trim().to_string()),
        base_url: base_url.clone(),
        api_key: api_key.clone(),
        wire_api,
        models: requested_models,
    };
    runtime_source.validate().map_err(|_| {
        ImportItemError::new(error_codes::SOURCE_INVALID, "imported source is invalid")
    })?;
    let discover_models = discover_models || runtime_source.models.is_empty();
    let mut protocol_config = existing
        .as_ref()
        .map(|source| source.protocol_config.clone())
        .unwrap_or_default();
    let (detected_model_prices, protocol_bindings) = if discover_models {
        let discovery = discover_source_models_and_protocol_bindings(&runtime_source, &[])
            .await
            .map_err(|_| {
                ImportItemError::new(
                    error_codes::SOURCE_MODEL_DISCOVERY_FAILED,
                    "source model discovery failed",
                )
            })?;
        let mut protocol_bindings = Vec::new();
        let mut detected_model_prices = BTreeMap::new();
        discovery.apply_catalog(
            &mut runtime_source.base_url,
            &mut runtime_source.models,
            &mut protocol_bindings,
            &mut protocol_config,
            &mut detected_model_prices,
        );
        (detected_model_prices, protocol_bindings)
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
            error_codes::MODELS_REQUIRED,
            "models are required when discovery is disabled",
        ));
    };
    let mut imported_source = imported_source_record(
        &import_item,
        runtime_source,
        secret_ref,
        existing.as_ref(),
        protocol_config,
        protocol_bindings,
        detected_model_prices,
        discover_models.then(|| Utc::now().to_rfc3339()),
    );
    imported_source.in_pool |= add_to_pool;
    imported_source.validate_protocol_bindings().map_err(|_| {
        ImportItemError::new(
            error_codes::SOURCE_PROTOCOL_INVALID,
            "imported source protocol binding is invalid",
        )
    })?;
    persist_imported_source(state, &imported_source, &api_key, existing.as_ref()).await?;
    crate::diagnostics::breadcrumb("source-import", "item_completed", &[]);
    Ok(imported_source)
}

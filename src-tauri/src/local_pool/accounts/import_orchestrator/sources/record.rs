use crate::local_pool::models::ProviderSourceRecord;
use std::collections::BTreeMap;
use zenith_relay_core::accounts::ParsedImportItem;
use zenith_relay_core::{
    ApiModelPriceOverride, ProviderSource, SourceProtocolBinding, SourceProtocolConfig,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn imported_source_record(
    import_item: &ParsedImportItem,
    runtime_source: ProviderSource,
    secret_ref: String,
    existing: Option<&ProviderSourceRecord>,
    protocol_config: SourceProtocolConfig,
    protocol_bindings: Vec<SourceProtocolBinding>,
    detected_model_prices: BTreeMap<String, ApiModelPriceOverride>,
    tested_at: Option<String>,
) -> ProviderSourceRecord {
    let tested = tested_at.is_some();
    let mut source_record = ProviderSourceRecord {
        id: runtime_source.id,
        name: runtime_source.name,
        enabled: existing.as_ref().is_none_or(|source| source.enabled),
        in_pool: existing.as_ref().is_some_and(|source| source.in_pool),
        draining: existing.as_ref().is_some_and(|source| source.draining),
        base_url: runtime_source.base_url,
        secret_ref,
        pricing_provider: existing
            .as_ref()
            .and_then(|source| source.pricing_provider.clone()),
        official_provider_family: existing
            .as_ref()
            .and_then(|source| source.official_provider_family.clone()),
        wire_api: runtime_source.wire_api,
        protocol_config,
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
            .or(import_item.priority)
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
    source_record.normalize();
    source_record
}

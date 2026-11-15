use crate::local_pool::models::ProviderSourceRecord;
use std::collections::BTreeMap;
use zenith_relay_core::accounts::ParsedImportItem;
use zenith_relay_core::{
    ApiModelPriceOverride, ProviderSource, SourceProtocolBinding, SourceProtocolConfig,
};

pub(crate) struct ImportedSourceRecordInput<'a> {
    pub(crate) import_item: &'a ParsedImportItem,
    pub(crate) runtime_source: ProviderSource,
    pub(crate) secret_ref: String,
    pub(crate) existing: Option<&'a ProviderSourceRecord>,
    pub(crate) protocol_config: SourceProtocolConfig,
    pub(crate) protocol_bindings: Vec<SourceProtocolBinding>,
    pub(crate) detected_model_prices: BTreeMap<String, ApiModelPriceOverride>,
    pub(crate) tested_at: Option<String>,
}

pub(crate) fn imported_source_record(input: ImportedSourceRecordInput<'_>) -> ProviderSourceRecord {
    let ImportedSourceRecordInput {
        import_item,
        runtime_source,
        secret_ref,
        existing,
        protocol_config,
        protocol_bindings,
        detected_model_prices,
        tested_at,
    } = input;
    let tested = tested_at.is_some();
    let mut source_record = ProviderSourceRecord {
        id: runtime_source.id,
        name: runtime_source.name,
        enabled: existing.is_none_or(|source| source.enabled),
        in_pool: existing.is_some_and(|source| source.in_pool),
        draining: existing.is_some_and(|source| source.draining),
        base_url: runtime_source.base_url,
        secret_ref,
        pricing_provider: existing.and_then(|source| source.pricing_provider.clone()),
        official_provider_family: existing
            .and_then(|source| source.official_provider_family.clone()),
        wire_api: runtime_source.wire_api,
        protocol_config,
        protocol_bindings,
        models: runtime_source.models,
        proxy_id: existing.and_then(|source| source.proxy_id.clone()),
        allowed_models: existing
            .map(|source| source.allowed_models.clone())
            .unwrap_or_default(),
        excluded_models: existing
            .map(|source| source.excluded_models.clone())
            .unwrap_or_default(),
        priority: existing
            .map(|source| source.priority)
            .or(import_item.priority)
            .unwrap_or_default(),
        weight: existing.map_or(1, |source| source.weight),
        recovery_delay_seconds: existing.map_or(0, |source| source.recovery_delay_seconds),
        model_price_overrides: existing
            .map(|source| source.model_price_overrides.clone())
            .unwrap_or_default(),
        detected_model_prices,
        last_used_at: existing.and_then(|source| source.last_used_at.clone()),
        last_test_at: tested_at.or_else(|| existing.and_then(|source| source.last_test_at.clone())),
        last_test_status: tested
            .then(|| "ok".to_string())
            .or_else(|| existing.and_then(|source| source.last_test_status.clone())),
        last_error: if tested {
            None
        } else {
            existing.and_then(|source| source.last_error.clone())
        },
    };
    source_record.normalize();
    source_record
}

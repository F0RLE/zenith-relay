use crate::state::SourceRecord;
use std::collections::BTreeMap;
use zenith_relay_core::WireApi;

pub(crate) fn pooled_source(id: &str, model: &str) -> SourceRecord {
    SourceRecord {
        id: id.into(),
        name: "Synthetic source".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        base_url: "https://example.test/v1".into(),
        secret_ref: format!("source:{id}"),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_config: Default::default(),
        protocol_bindings: Vec::new(),
        models: vec![model.into()],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: BTreeMap::new(),
        detected_model_prices: BTreeMap::new(),
        last_error_code: None,
    }
}

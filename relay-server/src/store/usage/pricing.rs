#[cfg(test)]
use zenith_relay_core::pricing::{PricingCatalog, PricingContext};

#[cfg(test)]
use super::super::configuration::SourcePriceOverrides;
#[cfg(test)]
use std::collections::BTreeMap;
#[cfg(test)]
use zenith_relay_core::ApiModelPriceOverride;

#[cfg(test)]
pub(super) fn test_pricing_catalog() -> PricingCatalog {
    PricingCatalog::from_litellm_json(include_str!(
        "../../../../crates/relay-core/tests/fixtures/litellm-prices.json"
    ))
    .expect("pricing fixture must be valid")
}

#[cfg(test)]
pub(super) fn test_pricing_context(
    price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
    source_price_overrides: &SourcePriceOverrides,
) -> PricingContext {
    PricingContext::from_price_overrides(price_overrides, source_price_overrides)
}

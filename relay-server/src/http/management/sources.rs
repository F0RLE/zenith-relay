use super::{
    clean_label, default_weight, normalized_values, runtime_error, store_error, valid_weight,
    validate_secret, validation_error, vault_error, ManagementError,
};
use crate::state::{AppState, SourceRecord};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::SourceSummary;
use zenith_relay_core::{
    discover_source_with_protocol_config, normalize_model_price_overrides,
    source_points_to_gateway, ApiModelPriceOverride, ProviderSource, SourceDiscovery,
    SourceProtocolBinding, SourceProtocolConfig, WireApi,
};

use read::discover_models;
pub use read::list_sources;
pub use read::probe_source;
#[cfg(test)]
use read::source_discovery_error;
pub use read::source_stats;
pub use read::test_source;
use record::clear_source_catalog;
use record::ensure_not_server_self_source;
use record::find_source;
use record::normalize_pricing_identity;
use record::normalize_record_protocol_bindings;
use record::normalize_source_prices;
use record::source_record;
use record::source_summary;
use record::valid_recovery_delay;
use record::validate_source_record;
pub use write::create_source;
pub use write::delete_source;
pub use write::update_source;

mod policy;
mod read;
mod record;
mod write;

use policy::{source_dispatch_permission_changed, source_runtime_policy_compatible};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sources", get(list_sources).post(create_source))
        .route("/sources/{id}", patch(update_source).delete(delete_source))
        .route("/sources/{id}/test", post(test_source))
        .route("/sources/{id}/probe", post(probe_source))
        .route("/sources/{id}/stats", get(source_stats))
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

#[derive(Default, Deserialize)]
pub struct SourceStatsQuery {
    #[serde(default)]
    force: bool,
}

#[cfg(test)]
mod tests;

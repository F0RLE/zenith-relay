use super::{
    apply_source_policies_if_running, apply_source_policy_if_running, cleanup_created_secret,
    core_error, fence_runtime_candidates, refresh_active_codex_catalog_in_background,
    refresh_local_gateway_key_scope_if_running, sync_records_or_rollback,
};
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
    models::{LocalPoolSnapshot, ProviderSourceRecord},
    state::DesktopState,
    store::secret_store,
};
use chrono::Utc;
use serde::Deserialize;
use std::collections::BTreeMap;
use tauri::{AppHandle, State};
use uuid::Uuid;
use zenith_relay_core::{
    discover_source_with_protocol_config, normalize_model_ids, normalize_source_protocol_bindings,
    source_points_to_gateway, ApiModelPriceOverride, ProviderSource, SourceDiscovery,
    SourceProtocolBinding, SourceProtocolConfig, SourceProviderStats, WireApi,
};
#[cfg(test)]
use zenith_relay_core::{MessagesReasoningMode, SourceAdapter};

pub(crate) mod edit;
pub(crate) mod inspect;
mod support;
use support::apply_source_priorities;
use support::current_records;
use support::default_weight;
use support::detected_prices_for_upstream;
use support::empty_source_discovery;
pub(in crate::local_pool) use support::ensure_not_gateway_self_source;
use support::normalize_pricing_identity;
use support::responses_wire_api;
use support::source_catalog_visibility_changed;
use support::source_dispatch_configuration_changed;
use support::source_probe_matches;
use support::source_runtime_policy_compatible;
pub(crate) use support::validate_source_record;

type CommandResult<T> = std::result::Result<T, CommandError>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSourceInput {
    name: String,
    base_url: String,
    api_key: String,
    #[serde(default)]
    pricing_provider: Option<String>,
    #[serde(default)]
    official_provider_family: Option<String>,
    #[serde(default = "responses_wire_api")]
    wire_api: WireApi,
    #[serde(default)]
    protocol_bindings: Vec<SourceProtocolBinding>,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    draining: bool,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSourceInput {
    source_id: String,
    name: String,
    base_url: String,
    #[serde(default)]
    pricing_provider: Option<String>,
    #[serde(default)]
    official_provider_family: Option<String>,
    wire_api: WireApi,
    #[serde(default)]
    protocol_bindings: Option<Vec<SourceProtocolBinding>>,
    models: Vec<String>,
    #[serde(default)]
    in_pool: Option<bool>,
    #[serde(default)]
    draining: bool,
    #[serde(default)]
    allowed_models: Vec<String>,
    #[serde(default)]
    excluded_models: Vec<String>,
    priority: i32,
    #[serde(default)]
    source_priorities: BTreeMap<String, i32>,
    weight: u32,
    #[serde(default)]
    recovery_delay_seconds: u64,
    #[serde(default)]
    model_price_overrides: Option<BTreeMap<String, ApiModelPriceOverride>>,
}
#[cfg(test)]
mod tests;

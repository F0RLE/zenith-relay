use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection};
use serde::Serialize;
#[cfg(test)]
use std::collections::BTreeMap;
use std::{
    collections::HashMap,
    path::Path,
    sync::{atomic::AtomicU64, Mutex},
    time::Instant,
};
use zenith_relay_core::{
    pricing::PricingMetadata,
    protocol::{UsageBucket, UsageGroup, UsageQuery, UsageTotals},
    ApiEquivalentSummary, DefaultServiceTier, ErrorOrigin, ObservedServiceTier, RoutingDiagnostics,
    ToolUseDiagnostics, UsageEvent,
};
#[cfg(test)]
use zenith_relay_core::{ApiModelPriceOverride, ResponseAffinityBinding};

mod affinity;
mod lifecycle;
mod migrations;
mod queries;
mod record;
mod state;
mod usage;

use migrations::*;
use usage::{
    account_pricing_aggregates, apply_usage_totals_delta, is_unfiltered_all_time, sql_u64,
    usage_buckets, usage_filter, usage_groups, usage_log_from_row, usage_model_equivalents,
    usage_totals,
};

#[cfg(test)]
pub type SourcePriceOverrides = zenith_relay_core::SourceModelPriceOverrides;

pub struct TelemetryDb {
    connection: Mutex<Connection>,
    usage_revision: AtomicU64,
    api_equivalent_cache: Mutex<Option<CachedUsageEquivalents>>,
    quota_equivalent_cache: Mutex<Option<CachedQuotaEquivalents>>,
    usage_totals_cache: Mutex<Option<UsageTotals>>,
    open_duration_ms: f64,
}

#[derive(Clone)]
struct CachedUsageEquivalents {
    usage_revision: u64,
    pricing_revision: String,
    value: UsageEquivalents,
}

#[derive(Clone)]
struct CachedQuotaEquivalents {
    usage_revision: u64,
    pricing_revision: String,
    windows: Vec<(String, u64, u64)>,
    value: HashMap<String, ApiEquivalentSummary>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLog {
    pub id: i64,
    pub created_at: String,
    pub request_id: String,
    pub attempt: u16,
    pub source_id: String,
    pub candidate_id: Option<String>,
    pub account_id: Option<String>,
    pub client_context_id: Option<String>,
    pub routing: Option<RoutingDiagnostics>,
    pub requested_model: Option<String>,
    pub resolved_model: Option<String>,
    pub requested_reasoning_effort: Option<String>,
    pub effective_reasoning_effort: Option<String>,
    pub wire_api: String,
    pub service_tier: DefaultServiceTier,
    pub applied_service_tier: Option<ObservedServiceTier>,
    pub success: bool,
    pub http_status: u16,
    pub error_category: Option<String>,
    pub error_origin: Option<ErrorOrigin>,
    pub upstream_error: Option<zenith_relay_core::usage::UpstreamErrorDetails>,
    pub tool_use: Option<ToolUseDiagnostics>,
    pub latency_ms: u64,
    pub ttft_ms: Option<u64>,
    pub generation_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub cache_write_ttl: Option<String>,
    pub reasoning_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub api_equivalent: ApiEquivalentSummary,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CacheSession {
    pub client_context_id: String,
    pub started_at: String,
    pub touched_at: String,
    pub model: Option<String>,
    pub cache_write_ttl: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalUsagePage {
    pub events: Vec<UsageLog>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub total_pages: u32,
    pub totals: UsageTotals,
    pub models: Vec<UsageGroup>,
    pub pool_members: Vec<UsageGroup>,
    pub buckets: Vec<UsageBucket>,
    #[serde(default)]
    pub pricing: PricingMetadata,
}

#[derive(Clone, Default)]
pub struct UsageEquivalents {
    pub accounts: HashMap<String, ApiEquivalentSummary>,
    pub sources: HashMap<String, ApiEquivalentSummary>,
}

pub(crate) use zenith_relay_core::pricing::{PricingCatalog, PricingContext};
pub(crate) use zenith_relay_core::CatalogPriceResolver;

#[cfg(test)]
fn test_pricing_catalog() -> PricingCatalog {
    PricingCatalog::from_litellm_json(include_str!(
        "../../../../crates/relay-core/tests/fixtures/litellm-prices.json"
    ))
    .expect("pricing fixture must be valid")
}

#[cfg(test)]
fn test_pricing_context(
    price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
    source_price_overrides: &SourcePriceOverrides,
) -> PricingContext {
    PricingContext::from_price_overrides(price_overrides, source_price_overrides)
}

fn valid_performance_name(name: &str) -> bool {
    matches!(
        name,
        "native_startup"
            | "vault"
            | "sqlite"
            | "window"
            | "first_frame"
            | "interactive"
            | "full_snapshot"
            | "full_snapshot_native"
            | "mode_switch"
            | "page_open"
    )
}

impl TelemetryDb {
    fn lock_connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| LocalPoolError::new(ErrorCode::Io, "usage database lock poisoned"))
    }
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, "local database lock poisoned")
}

fn db_error(error: rusqlite::Error) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::Io, format!("local database error: {error}"))
}

fn io_error(error: std::io::Error) -> LocalPoolError {
    LocalPoolError::io(error)
}

#[cfg(test)]
mod tests;

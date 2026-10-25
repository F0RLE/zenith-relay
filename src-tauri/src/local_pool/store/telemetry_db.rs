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
mod forecast;
mod lifecycle;
mod maintenance;
mod migrations;
mod performance;
mod queries;
mod reader;
mod record;
mod state;
mod support;
mod usage;
mod weekly_capacity;

pub(crate) use weekly_capacity::{CapacityObservation, WeeklyCapacityQuery, WeeklyCapacityReport};

use migrations::*;
pub(crate) use performance::PerformanceSample;
use usage::{
    account_pricing_aggregates, apply_usage_totals_delta, is_unfiltered_all_time, sql_u64,
    usage_buckets, usage_filter, usage_log_from_row, usage_totals,
};

#[cfg(test)]
pub type SourcePriceOverrides = zenith_relay_core::SourceModelPriceOverrides;

pub struct TelemetryDb {
    connection: Mutex<Connection>,
    report_connection: Option<Mutex<Connection>>,
    usage_revision: AtomicU64,
    api_equivalent_cache: Mutex<Option<CachedUsageEquivalents>>,
    quota_equivalent_cache: Mutex<Option<CachedQuotaEquivalents>>,
    #[cfg(test)]
    quota_window_reads: AtomicU64,
    usage_filter_options_cache: Mutex<Option<CachedUsageFilterOptions>>,
    #[cfg(test)]
    usage_filter_option_reads: AtomicU64,
    usage_totals_cache: Mutex<Option<UsageTotals>>,
    usage_report_cache: Mutex<Option<CachedUsageReport>>,
    #[cfg(test)]
    usage_report_builds: AtomicU64,
    open_duration_ms: f64,
    quota_samples: Mutex<forecast::QuotaSamples>,
    capacity_seen: Mutex<HashMap<String, (u64, String, String, String)>>,
    capacity_revision: AtomicU64,
}

#[derive(Clone)]
struct CachedUsageReport {
    query_key: String,
    usage_revision: u64,
    pricing_revision: String,
    // Only summary projections; no events, diagnostics or filter inventories.
    page: LocalUsagePage,
}

const MAX_USAGE_REPORT_CACHE_BYTES: usize = 2 * 1024 * 1024;

impl CachedUsageReport {
    fn fits_cache(query_key: &str, pricing_revision: &str, page: &LocalUsagePage) -> bool {
        // Bound the extra retained copy, not the report returned to the caller.
        // Cloning these vectors allocates their length, not spare capacity.
        let mut bytes = std::mem::size_of::<Self>()
            .saturating_add(query_key.len())
            .saturating_add(pricing_revision.len())
            .saturating_add(
                page.pricing
                    .catalog_revision
                    .as_ref()
                    .map_or(0, String::len),
            )
            .saturating_add(
                page.buckets
                    .len()
                    .saturating_mul(std::mem::size_of::<UsageBucket>()),
            );
        for group in page.models.iter().chain(&page.pool_members) {
            bytes = bytes
                .saturating_add(std::mem::size_of::<UsageGroup>())
                .saturating_add(group.key.len())
                .saturating_add(group.label.as_ref().map_or(0, String::len));
            if bytes > MAX_USAGE_REPORT_CACHE_BYTES {
                return false;
            }
        }
        bytes <= MAX_USAGE_REPORT_CACHE_BYTES
    }
}

#[derive(Clone)]
struct CachedUsageFilterOptions {
    usage_revision: u64,
    from_ms: Option<u64>,
    to_ms: Option<u64>,
    options: UsageFilterOptions,
}

#[derive(Clone)]
struct CachedUsageEquivalents {
    usage_revision: u64,
    pricing_revision: String,
    equivalents: UsageEquivalents,
}

#[derive(Clone)]
struct CachedQuotaEquivalents {
    pricing_revision: String,
    windows: HashMap<String, (u64, u64)>,
    equivalents_by_account: HashMap<String, ApiEquivalentSummary>,
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
    pub transport: String,
    pub service_tier: DefaultServiceTier,
    pub applied_service_tier: Option<ObservedServiceTier>,
    pub success: bool,
    pub http_status: u16,
    pub error_category: Option<String>,
    pub error_origin: Option<ErrorOrigin>,
    pub upstream_error: Option<zenith_relay_core::usage::UpstreamErrorDetails>,
    pub image_usage: Option<zenith_relay_core::usage::ImageUsage>,
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
    pub api_equivalent_breakdown: zenith_relay_core::usage::ApiEquivalentBreakdown,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CacheSession {
    pub client_context_id: String,
    pub candidate_kind: String,
    pub candidate_id: String,
    pub started_at: String,
    pub touched_at: String,
    pub model: Option<String>,
    pub cache_write_ttl: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheSessionScope {
    pub client_context_id: String,
    pub candidate_kind: CacheSessionCandidateKind,
    pub candidate_id: String,
    pub model: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheSessionCandidateKind {
    Account,
    Source,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageFilterOptions {
    pub models: Vec<String>,
    pub pool_members: Vec<String>,
    pub wire_apis: Vec<String>,
    pub transports: Vec<String>,
    pub error_categories: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalUsagePage {
    pub events: Vec<UsageLog>,
    /// Present only for a bounded sends-only read. Summary totals are not
    /// computed; total/page counts are lower bounds, never exact report counts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workload: Option<zenith_relay_core::protocol::UsageWorkload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_options: Option<UsageFilterOptions>,
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

fn valid_performance_name(metric_name: &str) -> bool {
    matches!(
        metric_name,
        "native_startup"
            | "vault"
            | "sqlite"
            | "window"
            | "first_frame"
            | "interactive"
            | "full_snapshot"
            | "full_snapshot_native"
            | "snapshot_stage"
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

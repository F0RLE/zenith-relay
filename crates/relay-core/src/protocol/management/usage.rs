use crate::{
    ApiEquivalentSummary, CacheWriteTtl, DefaultServiceTier, ErrorOrigin, ObservedServiceTier,
    PricingMetadata, RoutingDiagnostics, ToolUseDiagnostics, WireApi,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub id: i64,
    pub request_id: String,
    #[serde(default = "default_attempt")]
    pub attempt: u16,
    pub candidate_kind: String,
    pub candidate_hint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<RoutingDiagnostics>,
    pub requested_model: Option<String>,
    pub resolved_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective_reasoning_effort: Option<String>,
    pub wire_api: WireApi,
    #[serde(default)]
    pub service_tier: DefaultServiceTier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_service_tier: Option<ObservedServiceTier>,
    pub success: bool,
    pub http_status: u16,
    pub error_category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_origin: Option<ErrorOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use: Option<ToolUseDiagnostics>,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_ttl: Option<CacheWriteTtl>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub api_equivalent: ApiEquivalentSummary,
    pub created_at_ms: u64,
}

fn default_attempt() -> u16 {
    1
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub requests: u64,
    pub successful_requests: u64,
    pub latency_ms: u64,
    pub ttft_ms: u64,
    pub ttft_samples: u64,
    pub generation_ms: u64,
    pub generation_samples: u64,
    pub generation_output_tokens: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cached_input_samples: u64,
    #[serde(default)]
    pub cache_write_input_tokens: u64,
    #[serde(default)]
    pub cache_write_input_samples: u64,
    pub reasoning_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub speed_output_tokens: u64,
    pub speed_duration_ms: u64,
    pub api_equivalent: ApiEquivalentSummary,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageGroup {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub totals: UsageTotals,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBucket {
    pub start_ms: u64,
    pub totals: UsageTotals,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePage {
    pub events: Vec<UsageSummary>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub total_pages: u32,
    #[serde(default)]
    pub totals: UsageTotals,
    #[serde(default)]
    pub models: Vec<UsageGroup>,
    #[serde(default)]
    pub pool_members: Vec<UsageGroup>,
    #[serde(default)]
    pub buckets: Vec<UsageBucket>,
    #[serde(default)]
    pub pricing: PricingMetadata,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageRange {
    Daily,
    Weekly,
    Monthly,
    Custom,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageQuery {
    #[serde(default)]
    pub page: u32,
    #[serde(default)]
    pub page_size: u32,
    pub range: Option<UsageRange>,
    pub from_ms: Option<u64>,
    pub to_ms: Option<u64>,
    pub bucket_ms: Option<u64>,
    pub model_query: Option<String>,
    pub source_or_account_query: Option<String>,
    pub wire_api: Option<WireApi>,
    pub success: Option<bool>,
    pub error_category: Option<String>,
    pub request_id_query: Option<String>,
    /// Include request event rows in the response. `None` preserves the
    /// legacy behavior for API clients that do not send the projection hint.
    #[serde(default)]
    pub include_events: Option<bool>,
    /// Include model aggregates in the response. `None` preserves the legacy
    /// behavior for API clients that do not send the projection hint.
    #[serde(default)]
    pub include_models: Option<bool>,
    /// Include pool-member aggregates in the response. `None` preserves the
    /// legacy behavior for API clients that do not send the projection hint.
    #[serde(default)]
    pub include_pool_members: Option<bool>,
}

impl UsageQuery {
    pub fn normalize_pagination(&mut self) {
        self.page = self.page.max(1);
        self.page_size = if self.page_size == 0 {
            50
        } else {
            self.page_size.clamp(1, 200)
        };
        self.bucket_ms = self.bucket_ms.filter(|value| *value >= 60_000);
    }

    pub fn includes_models(&self) -> bool {
        self.include_models != Some(false)
    }

    pub fn includes_events(&self) -> bool {
        self.include_events != Some(false)
    }

    pub fn includes_pool_members(&self) -> bool {
        self.include_pool_members != Some(false)
    }
}

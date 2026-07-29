mod api_equivalent;

pub use api_equivalent::{
    api_model_price, api_pricing_revision, estimate_api_equivalent,
    estimate_api_equivalent_with_price_override, normalize_model_price_overrides, ApiModelPrice,
    ApiModelPriceOverride, MAX_MODEL_PRICE_MICRO_USD_PER_MILLION,
};

use crate::{quota::QuotaSnapshot, DefaultServiceTier, RoutingDiagnostics, WireApi};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub type UsageCallback = Arc<dyn Fn(UsageEvent) + Send + Sync>;

/// A provider-neutral monetary value attached to measured token usage.
///
/// The OpenAI catalog is one way to produce this value; quota calibration only
/// consumes the value and does not know how the provider price was obtained.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageValue {
    pub micro_usd: u64,
    pub priced_tokens: u64,
    pub unpriced_tokens: u64,
}

impl UsageValue {
    pub fn merge(&mut self, other: Self) {
        self.micro_usd = self.micro_usd.saturating_add(other.micro_usd);
        self.priced_tokens = self.priced_tokens.saturating_add(other.priced_tokens);
        self.unpriced_tokens = self.unpriced_tokens.saturating_add(other.unpriced_tokens);
    }
}

/// Compatibility name used by the management and desktop DTOs. New provider
/// code should use `UsageValue` so it does not imply an OpenAI-only source.
pub type ApiEquivalentSummary = UsageValue;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEvent {
    pub request_id: String,
    pub attempt: u16,
    pub local_key_id: String,
    pub source_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing: Option<RoutingDiagnostics>,
    pub requested_model: Option<String>,
    pub resolved_model: Option<String>,
    pub wire_api: WireApi,
    #[serde(default)]
    pub service_tier: DefaultServiceTier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_service_tier: Option<DefaultServiceTier>,
    pub success: bool,
    pub http_status: u16,
    pub error_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub consecutive_failures: Option<u32>,
    pub latency_ms: u64,
    pub ttft_ms: Option<u64>,
    pub generation_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    pub cache_write_input_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_snapshot: Option<QuotaSnapshot>,
}

impl UsageEvent {
    pub fn affects_account_state(&self) -> bool {
        if self.account_id.is_none() || self.success {
            return false;
        }
        !matches!(
            self.error_category.as_deref(),
            Some(
                "client_cancelled"
                    | "response_affinity_miss"
                    | "response_incomplete"
                    | "upstream_cancelled"
                    | "upstream_previous_response_not_found"
                    | "upstream_tool_call_mismatch"
                    | "upstream_context_too_large"
                    | "upstream_encrypted_content_invalid"
                    | "upstream_instructions_required"
                    | "upstream_content_policy"
                    | "upstream_payload_too_large"
                    | "upstream_unsupported_request"
                    | "upstream_websocket_unsupported"
                    | "upstream_invalid_request"
                    | "upstream_model_not_found"
                    | "upstream_model_unsupported"
                    | "upstream_usage_not_included"
                    | "image_generation_not_enabled"
            )
        )
    }
}

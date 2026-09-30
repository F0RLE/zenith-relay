use super::{ObservedServiceTier, ToolUseDiagnostics};
use crate::error_codes;
use crate::quota::QuotaSnapshot;
use crate::{DefaultServiceTier, RoutingDiagnostics, WireApi};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorOrigin {
    Provider,
    Account,
    Relay,
}

impl ErrorOrigin {
    pub fn for_category(self, category: &str) -> Self {
        if relay_error_category(category) || adapter_error_category_is_relay(category) {
            return Self::Relay;
        }
        // Origin identifies the selected route. Whether an upstream failure
        // affects account health is a separate decision in affects_account_state.
        self
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Account => "account",
            Self::Relay => "relay",
        }
    }
}

impl std::str::FromStr for ErrorOrigin {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "provider" => Ok(Self::Provider),
            "account" => Ok(Self::Account),
            "relay" => Ok(Self::Relay),
            _ => Err(()),
        }
    }
}

define_usage_request_contract! {
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
    /// Transient credential provenance for desktop account-state handling.
    ///
    /// It is deliberately excluded from persisted/exported usage. The desktop
    /// callback uses it only to make a delayed 401 a no-op when a newer OAuth
    /// credential generation is already stored for the same account.
    #[serde(skip)]
    pub account_token_generation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_context_id: Option<String>,
    #[serde(default)]
    pub tool_use: ToolUseDiagnostics,
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
    /// Exact retention durations reported by the upstream usage payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_ttl: Option<String>,
    pub reasoning_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quota_snapshot: Option<QuotaSnapshot>,
}
}

impl UsageEvent {
    /// Attributes a failed attempt to the component that produced its error.
    /// A Relay-origin error was constructed locally; otherwise the selected
    /// account or API source is responsible for the upstream result.
    pub fn error_origin(&self) -> Option<ErrorOrigin> {
        if self.success || self.error_category.is_none() {
            return None;
        }
        let category = self.error_category.as_deref().unwrap_or_default();
        let route_origin = if self.account_id.is_some() {
            ErrorOrigin::Account
        } else {
            ErrorOrigin::Provider
        };
        Some(route_origin.for_category(category))
    }

    pub fn affects_account_state(&self) -> bool {
        if self.account_id.is_none() || self.success {
            return false;
        }
        self.error_category
            .as_deref()
            .is_none_or(crate::gateway::failure_category_affects_account_state)
    }
}

fn relay_error_category(category: &str) -> bool {
    matches!(
        category,
        error_codes::INVALID_REQUEST
            | error_codes::MODEL_NOT_FOUND
            | error_codes::NO_ELIGIBLE_SOURCE
            | error_codes::ALL_SOURCES_TEMPORARILY_UNAVAILABLE
            | error_codes::ALL_SOURCES_COOLING_DOWN
            | "adapter_websocket_not_supported"
            | error_codes::CLIENT_CANCELLED
            | "client_websocket"
            | error_codes::RESPONSE_AFFINITY_MISS
            | error_codes::STREAM_EVENT_TOO_LARGE
    )
}

fn adapter_error_category_is_relay(category: &str) -> bool {
    category.starts_with("adapter_") && !category.starts_with("adapter_upstream_")
}

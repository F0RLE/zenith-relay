use super::{ObservedServiceTier, ToolUseDiagnostics};
use crate::error_codes;
use crate::quota::QuotaSnapshot;
use crate::{DefaultServiceTier, RoutingDiagnostics, WireApi};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageTransport {
    #[default]
    Http,
    Websocket,
}

impl UsageTransport {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Websocket => "websocket",
        }
    }
}

impl std::str::FromStr for UsageTransport {
    type Err = ();

    fn from_str(transport_name: &str) -> Result<Self, Self::Err> {
        match transport_name {
            "http" => Ok(Self::Http),
            "websocket" => Ok(Self::Websocket),
            _ => Err(()),
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorOrigin {
    Provider,
    Account,
    Relay,
}

impl ErrorOrigin {
    pub fn for_category(self, category: &str) -> Self {
        if category.starts_with("codex_background_blocked_")
            || relay_error_category(category)
            || adapter_error_category_is_relay(category)
        {
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

    pub const fn label(self) -> &'static str {
        match self {
            Self::Provider => "Provider",
            Self::Account => "Account",
            Self::Relay => "Relay",
        }
    }

    /// Prefixes user-visible error text with the selected source, replacing a
    /// stale source label if the message was already tagged elsewhere.
    pub fn prefix_message(self, message: &str) -> String {
        let mut message = message.trim_start();
        while let Some(unprefixed) = [Self::Provider, Self::Account, Self::Relay]
            .into_iter()
            .find_map(|origin| {
                let label_length = origin.label().len();
                let label = message.get(..label_length)?;
                if !label.eq_ignore_ascii_case(origin.label()) {
                    return None;
                }
                message
                    .get(label_length..)?
                    .strip_prefix(':')
                    .map(str::trim_start)
            })
        {
            message = unprefixed;
        }
        format!("{}: {message}", self.label())
    }
}

impl std::str::FromStr for ErrorOrigin {
    type Err = ();

    fn from_str(origin_name: &str) -> Result<Self, Self::Err> {
        match origin_name {
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
    pub fn is_request_completion(&self) -> bool {
        self.routing
            .as_ref()
            .and_then(|routing| routing.request_timing.as_ref())
            .is_some_and(|timing| timing.projection_only)
    }

    pub(crate) fn observe_response_metadata(&mut self, payload: &serde_json::Value) {
        // Responses echoes the applied cache policy outside `usage`, including
        // response.created/completed SSE envelopes. Do not infer it from the
        // request preference, transport, or model name.
        for response in std::iter::successors(Some(payload), |item| item.get("response")).take(3) {
            if let Some(ttl) = response
                .pointer("/prompt_cache_options/ttl")
                .and_then(serde_json::Value::as_str)
                .and_then(super::normalize_reported_cache_ttls)
            {
                self.cache_write_ttl = Some(ttl);
            }
        }
        if let Some(cache) = self
            .routing
            .as_mut()
            .and_then(|routing| routing.cache_context.as_mut())
        {
            for response in
                std::iter::successors(Some(payload), |item| item.get("response")).take(3)
            {
                if let Some(diagnostics) = response.get("prompt_cache_diagnostics")
                    && let Ok(diagnostics) =
                        super::ProviderCacheDiagnostics::deserialize(diagnostics)
                {
                    cache.provider = Some(diagnostics);
                }
                if response.get("status").and_then(serde_json::Value::as_str) == Some("completed")
                    && let Some(id) = response
                        .get("id")
                        .and_then(serde_json::Value::as_str)
                        .filter(|id| {
                            !id.is_empty()
                                && id.len() <= 256
                                && id.bytes().all(|byte| {
                                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')
                                })
                        })
                {
                    cache.provider_response_id = Some(id.to_owned());
                }
            }
        }
        let model = [
            "/response/model",
            "/message/model",
            "/model",
            "/modelVersion",
        ]
        .into_iter()
        .find_map(|path| payload.pointer(path).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|model| crate::catalog::is_valid_model_token(model));
        if let (Some(routing), Some(model)) = (self.routing.as_mut(), model) {
            if routing.upstream_response_model.as_deref() == Some(model) {
                return;
            }
            let (sanitized, redacted, truncated) = super::upstream_error::sanitize_message(model);
            if !redacted && !truncated && sanitized == model {
                routing.upstream_response_model = Some(sanitized);
            }
        }
    }

    /// A successful wake remains owned by its automation until quota verification.
    pub fn is_natural_account_use(&self) -> bool {
        self.success
            && self.account_id.is_some()
            && self.error_category.as_deref() != Some("codex_wake")
    }

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
        if self.account_id.is_none()
            || self.success
            || self.is_basis_points_transport_failure()
            || self
                .routing
                .as_ref()
                .and_then(|routing| routing.endpoint_kind.as_deref())
                == Some("chatgpt_siwc")
        {
            return false;
        }
        self.error_category
            .as_deref()
            .is_none_or(crate::gateway::failure_category_affects_account_state)
    }

    pub(crate) fn is_basis_points_transport_failure(&self) -> bool {
        !self.success
            && crate::gateway::basis_points_transport_rejected(
                self.http_status,
                self.error_category.as_deref(),
            )
            && self
                .routing
                .as_ref()
                .and_then(|routing| routing.endpoint_kind.as_deref())
                == Some("excel_basis_points")
    }
}

fn relay_error_category(category: &str) -> bool {
    matches!(
        category,
        error_codes::INVALID_REQUEST
            | error_codes::HTTP_HEADERS_TIMEOUT
            | error_codes::HTTP_SEND_START_TIMEOUT
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

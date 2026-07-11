use super::Capabilities;
use crate::{
    accounts::AccountAuthState,
    automations::{WakeHistory, WakeTask},
    quota::{QuotaSnapshot, Subscription},
    WireApi,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub server_id: String,
    pub started_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTargetSummary {
    pub kind: String,
    pub connected: bool,
    pub origin: Option<String>,
    pub server_id: Option<String>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySummary {
    pub running: bool,
    pub base_url: String,
    pub candidate_count: usize,
    pub visible_model_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSummary {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub draining: bool,
    pub base_url: String,
    pub wire_api: WireApi,
    pub models: Vec<String>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub priority: i32,
    pub weight: u32,
    pub secret_available: bool,
    pub last_error_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub id: String,
    pub label: String,
    pub identity_hint: String,
    pub enabled: bool,
    pub draining: bool,
    pub auth_state: AccountAuthState,
    pub health: String,
    pub models: Vec<String>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub priority: i32,
    pub weight: u32,
    pub subscription: Subscription,
    pub quota: QuotaSnapshot,
    pub secret_available: bool,
    pub last_error_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeySummary {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub source_ids: Option<Vec<String>>,
    pub account_ids: Option<Vec<String>>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub model_prefix: Option<String>,
    pub created_at_ms: u64,
    pub last_used_at_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStateSnapshot {
    pub schema_version: u32,
    pub runtime_target: RuntimeTargetSummary,
    pub gateway: GatewaySummary,
    pub platform: String,
    pub capabilities: Capabilities,
    pub sources: Vec<SourceSummary>,
    pub accounts: Vec<AccountSummary>,
    pub keys: Vec<KeySummary>,
    pub automations: Vec<WakeTask>,
    pub wake_history: Vec<WakeHistory>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub id: i64,
    pub request_id: String,
    pub local_key_id: String,
    pub candidate_kind: String,
    pub candidate_hint: String,
    pub requested_model: Option<String>,
    pub resolved_model: Option<String>,
    pub wire_api: WireApi,
    pub success: bool,
    pub http_status: u16,
    pub error_category: Option<String>,
    pub latency_ms: u64,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub created_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePage {
    pub events: Vec<UsageSummary>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub total_pages: u32,
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
    pub model_query: Option<String>,
    pub source_or_account_query: Option<String>,
    pub local_key_query: Option<String>,
    pub wire_api: Option<WireApi>,
    pub success: Option<bool>,
    pub error_category: Option<String>,
    pub request_id_query: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayDiagnostic {
    pub stream: bool,
    pub model: String,
    pub latency_ms: u64,
    pub bytes_received: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub stage: String,
    pub retryable: bool,
    pub request_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ErrorEnvelope {
    pub error: ApiError,
}

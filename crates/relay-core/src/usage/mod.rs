use crate::WireApi;
use serde::Serialize;
use std::sync::Arc;

pub type UsageCallback = Arc<dyn Fn(UsageEvent) + Send + Sync>;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageEvent {
    pub request_id: String,
    pub attempt: u16,
    pub local_key_id: String,
    pub source_id: String,
    pub requested_model: Option<String>,
    pub resolved_model: Option<String>,
    pub wire_api: WireApi,
    pub success: bool,
    pub http_status: u16,
    pub error_category: Option<String>,
    pub latency_ms: u64,
    pub ttft_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
}

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UiState {
    pub(super) provider_active: bool,
    pub(super) codex_running: bool,
    pub(super) has_saved_api_key: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TopUpIntentData {
    pub(super) bot_url: Option<String>,
    pub(super) url: Option<String>,
    pub(super) start_parameter: Option<String>,
    pub(super) start_payload: Option<String>,
    pub(super) code: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ApiEnvelope<T> {
    pub(super) data: T,
}

#[derive(Deserialize)]
pub(super) struct ModelsResponse {
    pub(super) data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
pub(super) struct ModelEntry {
    pub(super) id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PreparedTopUpAmount {
    pub(super) amount_cents: i64,
    pub(super) amount_usd: f64,
    pub(super) valid: bool,
}

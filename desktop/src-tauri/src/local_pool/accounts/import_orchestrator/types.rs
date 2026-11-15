use super::ImportAuthMode;
use crate::local_pool::accounts::import_session::ImportSession;
use serde::{Deserialize, Serialize};
use zenith_relay_core::accounts::ImportPreview;

fn default_true() -> bool {
    true
}

pub(in crate::local_pool::accounts) const MAX_ACCOUNT_LABEL_BYTES: usize = 128;

pub(in crate::local_pool::accounts) const MAX_MODELS: usize = zenith_relay_core::MAX_MODEL_LIST_LEN;

pub(in crate::local_pool::accounts) const DEFAULT_OPENAI_SOURCE_URL: &str =
    "https://api.openai.com/v1";

pub(in crate::local_pool::accounts) const MAX_ACCOUNT_PROFILE_RESPONSE_BYTES: usize = 256 * 1024;

pub(in crate::local_pool::accounts) const ACCOUNT_IMPORT_PROGRESS_EVENT: &str =
    "relay-account-import-progress";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareAccountImportInput {
    pub(in crate::local_pool::accounts) session_id: String,
    #[serde(default)]
    pub(in crate::local_pool::accounts) probe_quota: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfirmAccountImportInput {
    pub(in crate::local_pool::accounts) session_id: String,
    pub(in crate::local_pool::accounts) selected_item_ids: Vec<String>,
    #[serde(default)]
    pub(in crate::local_pool::accounts) add_to_pool: bool,
    #[serde(default = "default_true")]
    pub(in crate::local_pool::accounts) discover_models: bool,
    #[serde(default)]
    pub(in crate::local_pool::accounts) probe_quota: bool,
    #[serde(default)]
    pub(in crate::local_pool::accounts) models: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSessionResponse {
    pub session_id: String,
    pub created_at_ms: u64,
    pub prepared: bool,
    pub preview: ImportPreview,
}

impl From<ImportSession> for ImportSessionResponse {
    fn from(session: ImportSession) -> Self {
        Self {
            session_id: session.session_id,
            created_at_ms: session.created_at_ms,
            prepared: session.prepared,
            preview: session.preview,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportItemStatus {
    Succeeded,
    Failed,
}

#[derive(Clone)]
pub(in crate::local_pool::accounts) struct ImportRowContext {
    pub(in crate::local_pool::accounts) label: String,
    pub(in crate::local_pool::accounts) auth_mode: ImportAuthMode,
    pub(in crate::local_pool::accounts) selectable: bool,
    pub(in crate::local_pool::accounts) plan: Option<String>,
    pub(in crate::local_pool::accounts) subscription_active_until_ms: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(in crate::local_pool::accounts) struct AccountImportProgressEvent {
    pub(in crate::local_pool::accounts) session_id: String,
    pub(in crate::local_pool::accounts) completed: usize,
    pub(in crate::local_pool::accounts) total: usize,
    pub(in crate::local_pool::accounts) succeeded: usize,
    pub(in crate::local_pool::accounts) failed: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::local_pool::accounts) current_label: Option<String>,
}

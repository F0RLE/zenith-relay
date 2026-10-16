use serde::{Deserialize, Serialize};

/// Redacted trash listing. Credentials and operational records stay in storage.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashedAccountSummary {
    pub id: String,
    pub label: String,
    pub trashed_at_ms: u64,
}

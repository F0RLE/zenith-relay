use crate::local_pool::error::ErrorCode;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogRefreshClient {
    ClaudeDesktop,
    #[serde(rename = "opencode")]
    OpenCode,
}

impl CatalogRefreshClient {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeDesktop => "claude_desktop",
            Self::OpenCode => "opencode",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCatalogRefreshFailure {
    pub code: ErrorCode,
    pub recorded_at_ms: u64,
}

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCapabilities {
    pub features: BTreeSet<String>,
    /// Metadata for future connection/pool UI; it never grants execution rights.
    #[serde(default)]
    pub account_connectors: Vec<crate::providers::connectors::AccountConnectorDescriptor>,
}

impl DesktopCapabilities {
    pub fn current() -> Self {
        let features = [
            "accounts",
            "account_connectors_v1",
            "account_trash_v1",
            "account_batch_import",
            "account_batch_import_creation_status",
            "account_import_to_pool",
            "account_export",
            "account_identity_reveal",
            "sources",
            "source_protocols_v1",
            "gemini_interactions_native_v1",
            "quota",
            "models",
            "model_order_reset",
            "model_pricing",
            "usage",
            "local_gateway",
            "profile_attach",
            "diagnostics",
            "wake_tasks",
            "account_proxies",
            "runtime_routing",
            "rotation_v3",
            "configuration_presets",
            "codex_background_tasks",
            "codex_websockets",
            "chatgpt_retry_until_available",
            "route_recovery_v1",
            "block_degraded_routes",
            "tool_policy_v1",
            "images",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        Self {
            features,
            account_connectors: crate::providers::connectors::available_account_connectors(),
        }
    }
}

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopCapabilities {
    pub features: BTreeSet<String>,
}

impl DesktopCapabilities {
    pub fn current() -> Self {
        let features = [
            "accounts",
            "account_trash_v1",
            "account_batch_import",
            "account_batch_import_creation_status",
            "account_import_to_pool",
            "account_export",
            "account_identity_reveal",
            "sources",
            "source_protocols_v1",
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
            "request_timezone_v1",
            "images",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        Self { features }
    }
}

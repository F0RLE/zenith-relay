use crate::WireApi;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const CURRENT_PROTOCOL_VERSION: u16 = 2;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Feature {
    Accounts,
    AccountBatchImport,
    AccountBatchImportCreationStatus,
    AccountImportToPool,
    AccountExport,
    AccountIdentityReveal,
    Sources,
    SourceProtocols,
    Quota,
    Models,
    ModelOrderReset,
    ModelPricing,
    Usage,
    LocalGateway,
    ProfileAttach,
    Diagnostics,
    WakeTasks,
    Backups,
    AccountProxies,
    RuntimeRouting,
    Rotation,
    ConfigurationPresets,
    ProfileKeyRotation,
    CodexBackgroundTasks,
    CodexWebsockets,
    ChatgptRetryUntilAvailable,
    RouteRecovery,
    ToolPolicy,
    Images,
}

impl Feature {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accounts => "accounts",
            Self::AccountBatchImport => "account_batch_import",
            Self::AccountBatchImportCreationStatus => "account_batch_import_creation_status",
            Self::AccountImportToPool => "account_import_to_pool",
            Self::AccountExport => "account_export",
            Self::AccountIdentityReveal => "account_identity_reveal",
            Self::Sources => "sources",
            Self::SourceProtocols => "source_protocols_v1",
            Self::Quota => "quota",
            Self::Models => "models",
            Self::ModelOrderReset => "model_order_reset",
            Self::ModelPricing => "model_pricing",
            Self::Usage => "usage",
            Self::LocalGateway => "local_gateway",
            Self::ProfileAttach => "profile_attach",
            Self::Diagnostics => "diagnostics",
            Self::WakeTasks => "wake_tasks",
            Self::Backups => "backups",
            Self::AccountProxies => "account_proxies",
            Self::RuntimeRouting => "runtime_routing",
            // Existing servers and desktop clients negotiate this wire token.
            Self::Rotation => "rotation_v2",
            Self::ConfigurationPresets => "configuration_presets",
            Self::ProfileKeyRotation => "profile_key_rotation",
            Self::CodexBackgroundTasks => "codex_background_tasks",
            Self::CodexWebsockets => "codex_websockets",
            // The wire feature name predates support for all four text protocols.
            Self::ChatgptRetryUntilAvailable => "chatgpt_retry_until_available",
            Self::RouteRecovery => "route_recovery_v1",
            Self::ToolPolicy => "tool_policy_v1",
            Self::Images => "images",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub protocol_version: u16,
    pub compatibility_min_client: u16,
    pub server_name: String,
    pub server_id: String,
    pub identity_fingerprint: String,
    pub server_managed_by_user: bool,
    pub features: BTreeSet<String>,
    pub supported_wire_apis: Vec<WireApi>,
    pub supports_accounts: bool,
    pub supports_sources: bool,
    pub supports_quota: bool,
    pub supports_usage: bool,
    pub supports_local_gateway: bool,
    pub supports_profile_attach: bool,
    pub supports_wake_tasks: bool,
}

impl Capabilities {
    pub fn desktop_local() -> Self {
        let mut capabilities = Self::personal_server("desktop-local", "desktop-local-runtime");
        capabilities.server_name = "Zenith Relay Desktop".to_string();
        capabilities.server_managed_by_user = false;
        capabilities
            .features
            .insert(Feature::ProfileAttach.as_str().to_string());
        capabilities.supports_profile_attach = true;
        capabilities
    }

    pub fn personal_server(
        server_id: impl Into<String>,
        identity_fingerprint: impl Into<String>,
    ) -> Self {
        let features = [
            Feature::Accounts,
            Feature::AccountBatchImport,
            Feature::AccountBatchImportCreationStatus,
            Feature::AccountImportToPool,
            Feature::AccountExport,
            Feature::AccountIdentityReveal,
            Feature::Sources,
            Feature::SourceProtocols,
            Feature::Quota,
            Feature::Models,
            Feature::ModelOrderReset,
            Feature::ModelPricing,
            Feature::Usage,
            Feature::LocalGateway,
            Feature::ProfileAttach,
            Feature::Diagnostics,
            Feature::WakeTasks,
            Feature::Backups,
            Feature::AccountProxies,
            Feature::RuntimeRouting,
            Feature::Rotation,
            Feature::ConfigurationPresets,
            Feature::ProfileKeyRotation,
            Feature::CodexBackgroundTasks,
            Feature::CodexWebsockets,
            Feature::ChatgptRetryUntilAvailable,
            Feature::RouteRecovery,
            Feature::ToolPolicy,
            Feature::Images,
        ]
        .into_iter()
        .map(|feature| feature.as_str().to_string())
        .collect();
        Self {
            protocol_version: CURRENT_PROTOCOL_VERSION,
            compatibility_min_client: CURRENT_PROTOCOL_VERSION,
            server_name: "Zenith Relay Server".to_string(),
            server_id: server_id.into(),
            identity_fingerprint: identity_fingerprint.into(),
            server_managed_by_user: true,
            features,
            supported_wire_apis: vec![
                WireApi::Responses,
                WireApi::ChatCompletions,
                WireApi::Messages,
                WireApi::Gemini,
            ],
            supports_accounts: true,
            supports_sources: true,
            supports_quota: true,
            supports_usage: true,
            supports_local_gateway: true,
            supports_profile_attach: true,
            supports_wake_tasks: true,
        }
    }

    pub fn supports(&self, feature: Feature) -> bool {
        self.features.contains(feature.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::{Capabilities, Feature};

    #[test]
    fn route_recovery_has_a_distinct_capability_from_the_legacy_chatgpt_setting() {
        let server = Capabilities::personal_server("server", "fingerprint");
        assert!(server.supports(Feature::RouteRecovery));
        assert!(server.supports(Feature::ChatgptRetryUntilAvailable));
        let mut legacy = server;
        legacy.features.remove(Feature::RouteRecovery.as_str());
        assert!(!legacy.supports(Feature::RouteRecovery));
        assert!(legacy.supports(Feature::ChatgptRetryUntilAvailable));
    }
}

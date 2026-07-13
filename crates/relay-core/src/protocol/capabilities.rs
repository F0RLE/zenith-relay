use crate::WireApi;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const CURRENT_PROTOCOL_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Feature {
    Accounts,
    AccountBatchImport,
    AccountExport,
    AccountIdentityReveal,
    Sources,
    Quota,
    Models,
    Usage,
    LocalGateway,
    Keys,
    ProfileAttach,
    Diagnostics,
    WakeTasks,
    Backups,
    AccountProxies,
    FreeAccountPolicy,
}

impl Feature {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accounts => "accounts",
            Self::AccountBatchImport => "account_batch_import",
            Self::AccountExport => "account_export",
            Self::AccountIdentityReveal => "account_identity_reveal",
            Self::Sources => "sources",
            Self::Quota => "quota",
            Self::Models => "models",
            Self::Usage => "usage",
            Self::LocalGateway => "local_gateway",
            Self::Keys => "keys",
            Self::ProfileAttach => "profile_attach",
            Self::Diagnostics => "diagnostics",
            Self::WakeTasks => "wake_tasks",
            Self::Backups => "backups",
            Self::AccountProxies => "account_proxies",
            Self::FreeAccountPolicy => "free_account_policy",
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
            Feature::AccountExport,
            Feature::AccountIdentityReveal,
            Feature::Sources,
            Feature::Quota,
            Feature::Models,
            Feature::Usage,
            Feature::LocalGateway,
            Feature::Keys,
            Feature::Diagnostics,
            Feature::WakeTasks,
            Feature::Backups,
            Feature::AccountProxies,
            Feature::FreeAccountPolicy,
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
            supported_wire_apis: vec![WireApi::Responses, WireApi::ChatCompletions],
            supports_accounts: true,
            supports_sources: true,
            supports_quota: true,
            supports_usage: true,
            supports_local_gateway: true,
            supports_profile_attach: false,
            supports_wake_tasks: true,
        }
    }

    pub fn supports(&self, feature: Feature) -> bool {
        self.features.contains(feature.as_str())
    }
}

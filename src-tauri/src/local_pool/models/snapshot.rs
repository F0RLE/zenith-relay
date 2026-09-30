use super::*;
use crate::platform::PlatformCapabilities;
use serde::Serialize;
use zenith_relay_core::automations::{WakeHistory, WakeTask};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTarget {
    pub kind: &'static str,
    pub connected: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPoolSnapshot {
    pub schema_version: u32,
    pub runtime_target: RuntimeTarget,
    pub gateway: GatewaySettings,
    pub platform: &'static str,
    pub capabilities: PlatformCapabilities,
    pub sources: Vec<ProviderSourceRecord>,
    pub accounts: Vec<LocalAccountRecord>,
    pub automations: Vec<WakeTask>,
    pub wake_history: Vec<WakeHistory>,
    pub warnings: Vec<String>,
}

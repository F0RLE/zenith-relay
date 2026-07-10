use crate::platform::PlatformCapabilities;
use serde::{Deserialize, Serialize};

pub const CURRENT_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_GATEWAY_PORT: u16 = 14998;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreMetadata {
    pub schema_version: u32,
}

impl Default for StoreMetadata {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BindScope {
    Localhost,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySettings {
    pub enabled: bool,
    pub bind_scope: BindScope,
    pub port: u16,
    pub client_host: String,
}

impl Default for GatewaySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_scope: BindScope::Localhost,
            port: DEFAULT_GATEWAY_PORT,
            client_host: "127.0.0.1".to_string(),
        }
    }
}

impl GatewaySettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.port < 1024 {
            return Err("gateway port must be between 1024 and 65535");
        }
        if self.client_host != "127.0.0.1" && self.client_host != "localhost" {
            return Err("local gateway host must be localhost or 127.0.0.1");
        }
        Ok(())
    }
}

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
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_validation_rejects_privileged_port_and_remote_host() {
        let mut settings = GatewaySettings::default();
        settings.port = 443;
        assert!(settings.validate().is_err());

        settings.port = DEFAULT_GATEWAY_PORT;
        settings.client_host = "0.0.0.0".to_string();
        assert!(settings.validate().is_err());
    }
}

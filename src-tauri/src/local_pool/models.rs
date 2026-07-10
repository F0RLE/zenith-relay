use crate::platform::PlatformCapabilities;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use zenith_relay_core::{
    accounts::AccountRecord,
    automations::{WakeAutomationState, WakeHistory, WakeTask},
    WireApi,
};

pub const CURRENT_SCHEMA_VERSION: u32 = 4;
pub const DEFAULT_GATEWAY_PORT: u16 = 14998;
pub const DEFAULT_MAX_RETRY_CANDIDATES: u8 = 3;
pub const DEFAULT_SESSION_AFFINITY_TTL_SECONDS: u64 = 3_600;
pub const MAX_LOCAL_ACCOUNTS: usize = 512;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BindScope {
    Localhost,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySettings {
    pub enabled: bool,
    pub bind_scope: BindScope,
    pub port: u16,
    pub client_host: String,
    #[serde(default = "default_max_retry_candidates")]
    pub max_retry_candidates: u8,
    #[serde(default = "default_session_affinity")]
    pub session_affinity: bool,
    #[serde(default = "default_session_affinity_ttl_seconds")]
    pub session_affinity_ttl_seconds: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSourceRecord {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub draining: bool,
    pub base_url: String,
    pub secret_ref: String,
    pub wire_api: WireApi,
    pub models: Vec<String>,
    #[serde(default)]
    pub allowed_models: Vec<String>,
    #[serde(default)]
    pub excluded_models: Vec<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_weight")]
    pub weight: u32,
    #[serde(default)]
    pub last_used_at: Option<String>,
    pub last_test_at: Option<String>,
    pub last_test_status: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalGatewayKeyRecord {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    pub secret_ref: String,
    #[serde(default)]
    pub source_ids: Option<Vec<String>>,
    #[serde(default)]
    pub account_ids: Option<Vec<String>>,
    #[serde(default)]
    pub allowed_models: Vec<String>,
    #[serde(default)]
    pub excluded_models: Vec<String>,
    #[serde(default)]
    pub model_prefix: Option<String>,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAccountRecord {
    pub account: AccountRecord,
    pub wire_api: WireApi,
    pub models: Vec<String>,
    #[serde(default)]
    pub allowed_models: Vec<String>,
    #[serde(default)]
    pub excluded_models: Vec<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_weight")]
    pub weight: u32,
    #[serde(default)]
    pub cooldowns: BTreeMap<String, u64>,
    #[serde(default)]
    pub consecutive_failures: u32,
}

impl LocalAccountRecord {
    pub fn normalize(&mut self) {
        self.account.label = self.account.label.trim().to_string();
        self.models = normalized_values(std::mem::take(&mut self.models));
        self.allowed_models = normalized_values(std::mem::take(&mut self.allowed_models));
        self.excluded_models = normalized_values(std::mem::take(&mut self.excluded_models));
        self.weight = self.weight.max(1);
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationRecords {
    pub tasks: Vec<WakeTask>,
    pub state: WakeAutomationState,
}

impl Default for AutomationRecords {
    fn default() -> Self {
        Self {
            tasks: Vec::new(),
            state: WakeAutomationState::new(1_024, 256)
                .expect("static wake automation bounds are valid"),
        }
    }
}

impl Default for GatewaySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_scope: BindScope::Localhost,
            port: DEFAULT_GATEWAY_PORT,
            client_host: "127.0.0.1".to_string(),
            max_retry_candidates: DEFAULT_MAX_RETRY_CANDIDATES,
            session_affinity: true,
            session_affinity_ttl_seconds: DEFAULT_SESSION_AFFINITY_TTL_SECONDS,
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
        if !(1..=8).contains(&self.max_retry_candidates) {
            return Err("max retry candidates must be between 1 and 8");
        }
        if !(60..=86_400).contains(&self.session_affinity_ttl_seconds) {
            return Err("session affinity TTL must be between 60 and 86400 seconds");
        }
        Ok(())
    }
}

impl ProviderSourceRecord {
    pub fn normalize(&mut self) {
        self.name = self.name.trim().to_string();
        self.base_url = self.base_url.trim().to_string();
        self.models = normalized_values(std::mem::take(&mut self.models));
        self.allowed_models = normalized_values(std::mem::take(&mut self.allowed_models));
        self.excluded_models = normalized_values(std::mem::take(&mut self.excluded_models));
        self.weight = self.weight.max(1);
    }
}

impl LocalGatewayKeyRecord {
    pub fn normalize(&mut self) {
        self.label = self.label.trim().to_string();
        self.source_ids = self.source_ids.take().map(normalized_values);
        self.account_ids = self.account_ids.take().map(normalized_values);
        self.allowed_models = normalized_values(std::mem::take(&mut self.allowed_models));
        self.excluded_models = normalized_values(std::mem::take(&mut self.excluded_models));
        self.model_prefix = self
            .model_prefix
            .take()
            .map(|prefix| prefix.trim().trim_matches('/').to_string())
            .filter(|prefix| !prefix.is_empty());
    }
}

fn normalized_values(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.to_ascii_lowercase()))
        .collect()
}

fn default_max_retry_candidates() -> u8 {
    DEFAULT_MAX_RETRY_CANDIDATES
}

fn default_session_affinity() -> bool {
    true
}

fn default_session_affinity_ttl_seconds() -> u64 {
    DEFAULT_SESSION_AFFINITY_TTL_SECONDS
}

fn default_weight() -> u32 {
    1
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
    pub sources: Vec<ProviderSourceRecord>,
    pub accounts: Vec<LocalAccountRecord>,
    pub keys: Vec<LocalGatewayKeyRecord>,
    pub automations: Vec<WakeTask>,
    pub wake_history: Vec<WakeHistory>,
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_validation_rejects_privileged_port_and_remote_host() {
        let mut settings = GatewaySettings {
            port: 443,
            ..GatewaySettings::default()
        };
        assert!(settings.validate().is_err());

        settings.port = DEFAULT_GATEWAY_PORT;
        settings.client_host = "0.0.0.0".to_string();
        assert!(settings.validate().is_err());

        settings.client_host = "127.0.0.1".to_string();
        settings.max_retry_candidates = 0;
        assert!(settings.validate().is_err());
        settings.max_retry_candidates = DEFAULT_MAX_RETRY_CANDIDATES;
        settings.session_affinity_ttl_seconds = 59;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn record_normalization_preserves_explicit_empty_key_scope() {
        let mut key = LocalGatewayKeyRecord {
            id: "key_1".into(),
            label: "  Scoped  ".into(),
            enabled: true,
            secret_ref: "key:key_1".into(),
            source_ids: Some(vec![" source_1 ".into(), "SOURCE_1".into()]),
            account_ids: Some(vec![" account_1 ".into(), "ACCOUNT_1".into()]),
            allowed_models: vec![" gpt-test ".into()],
            excluded_models: Vec::new(),
            model_prefix: Some(" /team/ ".into()),
            created_at: "2026-07-10T00:00:00Z".into(),
            last_used_at: None,
        };
        key.normalize();
        assert_eq!(key.label, "Scoped");
        assert_eq!(key.source_ids, Some(vec!["source_1".into()]));
        assert_eq!(key.account_ids, Some(vec!["account_1".into()]));
        assert_eq!(key.model_prefix.as_deref(), Some("team"));

        key.source_ids = Some(Vec::new());
        key.account_ids = Some(Vec::new());
        key.normalize();
        assert_eq!(key.source_ids, Some(Vec::new()));
        assert_eq!(key.account_ids, Some(Vec::new()));
    }
}

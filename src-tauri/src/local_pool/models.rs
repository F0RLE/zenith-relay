mod gateway;
mod participant;
mod snapshot;

pub(crate) use zenith_relay_core::normalize_model_ids as normalized_values;

pub use gateway::{BindScope, GatewaySettings};
#[cfg(test)]
pub(crate) use participant::synthetic_responses_source;
pub use participant::{
    AutomationRecords, LocalAccountRecord, LocalGatewayKeyRecord, ProviderSourceRecord,
    TrashedLocalAccount,
};
pub use snapshot::LocalPoolSnapshot;
pub use zenith_relay_core::accounts::AccountRouteGroup;

pub const CURRENT_SCHEMA_VERSION: u32 = 14;
pub const DEFAULT_GATEWAY_PORT: u16 = 14998;
pub use zenith_relay_core::protocol::{
    DEFAULT_MAX_RETRY_CANDIDATES, DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS,
};
pub const DEFAULT_CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS: u64 = 100;
pub const MIN_CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS: u64 = 100;
pub const MAX_CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS: u64 = 9_900;
pub const MAX_LOCAL_ACCOUNTS: usize = 1_024;

#[cfg(test)]
mod tests {
    use super::*;
    use zenith_relay_core::protocol::MIN_QUOTA_REQUEST_TIMEOUT_SECONDS;

    #[test]
    fn gateway_settings_ignore_retired_transport_flag_without_losing_current_settings() {
        let expected = GatewaySettings {
            enabled: true,
            port: 15432,
            ..GatewaySettings::default()
        };
        let canonical = serde_json::to_value(&expected).unwrap();
        assert!(canonical.get("basisPointsEnabled").is_none());
        for enabled in [false, true] {
            let mut legacy = canonical.clone();
            legacy["basisPointsEnabled"] = serde_json::json!(enabled);
            let restored: GatewaySettings = serde_json::from_value(legacy).unwrap();
            assert_eq!(restored, expected);
            assert_eq!(serde_json::to_value(restored).unwrap(), canonical);
        }
    }

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
    }

    #[test]
    fn gateway_validation_bounds_quota_policy() {
        let mut settings = GatewaySettings::default();
        assert!(settings.validate().is_ok());
        settings.quota_request_timeout_seconds = MIN_QUOTA_REQUEST_TIMEOUT_SECONDS - 1;
        assert!(settings.validate().is_err());
        settings.quota_request_timeout_seconds = MIN_QUOTA_REQUEST_TIMEOUT_SECONDS;
        assert!(settings.validate().is_ok());

        settings.chatgpt_interface_quota_reserve_basis_points = 0;
        assert!(settings.validate().is_ok());
        settings.chatgpt_interface_quota_reserve_basis_points = 99;
        assert!(settings.validate().is_err());
        settings.chatgpt_interface_quota_reserve_basis_points = 100;
        assert!(settings.validate().is_ok());
        settings.chatgpt_interface_quota_reserve_basis_points =
            MAX_CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS;
        assert!(settings.validate().is_ok());
    }
}

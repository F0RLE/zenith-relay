#[cfg(test)]
use super::credentials::StoredCodexCredentials;
#[cfg(test)]
use crate::local_pool::{error::ErrorCode, models::GatewaySettings};
#[cfg(test)]
use zenith_relay_core::protocol::ProxyMode;

pub(crate) mod check;

pub const COMMON_PROXY_SECRET_REF: &str = "proxy:common";
pub const PROXY_POOL_SECRET_REF: &str = "proxy:pool";
const PROXY_POOL_VERSION: u32 = 2;
const MAX_PROXY_POOL_ENTRIES: usize = 1_000;

pub(crate) fn is_proxy_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=80).contains(&bytes.len())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'-'))
}

mod pool;
mod refresh;
mod selection;

pub(crate) use pool::ProxyPool;
pub use pool::ProxyPoolSummary;
pub use refresh::ProxyRefreshClient;
#[cfg(test)]
use selection::choose_proxy_url;
pub use selection::{
    common_proxy_available, common_proxy_config, common_proxy_url, effective_proxy_config,
    effective_proxy_url, ensure_account_proxy, proxy_route_is_usable, proxy_route_status,
    proxy_status, ProxyRoute,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_proxy_overrides_common_and_missing_common_fails_closed() {
        let account = choose_proxy_url(Some("http://account.example:8080/"), false, true, || {
            panic!("common proxy must not be loaded for an account override")
        })
        .unwrap();
        assert_eq!(account.as_deref(), Some("http://account.example:8080/"));

        assert_eq!(
            choose_proxy_url(None, true, true, || panic!("common proxy must be bypassed")).unwrap(),
            None
        );
        let error = choose_proxy_url(None, false, true, || Ok(None)).unwrap_err();
        assert!(matches!(error.code, ErrorCode::SecretStoreUnavailable));
        assert_eq!(
            choose_proxy_url(None, false, false, || Ok(None)).unwrap(),
            None
        );
    }

    #[test]
    fn required_proxy_blocks_direct_account_egress() {
        let settings = GatewaySettings {
            account_proxy_required: true,
            ..Default::default()
        };
        assert!(ensure_account_proxy(&settings, None::<()>).is_err());
        assert!(ensure_account_proxy(&settings, Some(())).is_ok());
        assert_eq!(
            proxy_status(&settings, &credentials_without_proxy(), false),
            (ProxyMode::Direct, false)
        );
    }

    #[test]
    fn stored_proxy_is_deduplicated_redacted_and_automatically_shared() {
        let mut pool = ProxyPool::default();
        let values = vec![
            "host.example:8080:user:secret".to_string(),
            "http://user:secret@host.example:8080".to_string(),
        ];
        assert_eq!(pool.import(&values, 1).unwrap(), (1, 1));

        assert!(pool.assign_automatic("account-a").is_some());
        assert!(pool.assign_automatic("account-b").is_some());
        let summary = pool.summary();
        assert_eq!(summary.assigned, 1);
        assert_eq!(summary.entries[0].assigned_account_ids.len(), 2);
        assert_eq!(summary.entries[0].endpoint, "http://host.example:8080");
        assert!(!summary.entries[0].endpoint.contains("secret"));

        pool.release("account-a");
        assert_eq!(
            pool.summary().entries[0].assigned_account_ids,
            ["account-b"]
        );
    }

    #[test]
    fn legacy_proxy_pool_and_declared_location_are_preserved() {
        let legacy = r#"{
            "version": 1,
            "entries": [{
                "id": "proxy_old",
                "url": "http://user__cr.us%3Bregion.ca:secret@host.example:8080/",
                "assignedAccountId": "account-a",
                "createdAtMs": 1
            }]
        }"#;
        let pool = ProxyPool::from_json(legacy).unwrap();
        assert_eq!(pool.version, PROXY_POOL_VERSION);
        let summary = pool.summary();
        assert_eq!(summary.entries[0].assigned_account_ids, ["account-a"]);
        assert_eq!(summary.entries[0].country_code.as_deref(), Some("US"));
        assert_eq!(summary.entries[0].region.as_deref(), Some("ca"));
    }

    fn credentials_without_proxy() -> StoredCodexCredentials {
        StoredCodexCredentials::new(
            "account",
            "access".into(),
            Some("refresh".into()),
            None,
            None,
            0,
            0,
            None,
            Some("provider-account".into()),
            None,
            None,
            None,
            false,
        )
        .unwrap()
    }
}

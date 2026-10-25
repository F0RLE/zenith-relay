#[cfg(test)]
use super::credentials::StoredCodexCredentials;
#[cfg(test)]
use crate::local_pool::{error::ErrorCode, models::GatewaySettings};
#[cfg(test)]
use zenith_relay_core::protocol::ProxyMode;

pub(crate) mod check;
pub(crate) mod speed;

pub const COMMON_PROXY_SECRET_REF: &str = "proxy:common";
pub const PROXY_POOL_SECRET_REF: &str = "proxy:pool";
const PROXY_POOL_VERSION: u32 = 2;
const MAX_PROXY_POOL_ENTRIES: usize = 1_000;

pub(crate) fn is_proxy_id(proxy_id: &str) -> bool {
    let bytes = proxy_id.as_bytes();
    (1..=80).contains(&bytes.len())
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'_' | b'-'))
}

pub(crate) mod observations;
mod pool;
mod refresh;
mod selection;

pub use pool::ProxyPoolSummary;
pub(crate) use pool::{ProxyPool, ProxyRefreshScope};
pub use refresh::ProxyRefreshClient;
#[cfg(test)]
use selection::choose_proxy_url;
#[cfg(test)]
pub use selection::proxy_status;
pub(crate) use selection::{account_proxy_pool_id, common_proxy_pool_id};
pub use selection::{
    common_proxy_available, common_proxy_config, common_proxy_url, effective_proxy_config,
    effective_proxy_url, ensure_account_proxy, proxy_route_is_usable, proxy_route_status,
    ProxyRoute,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_names_round_trip_without_changing_routes_and_can_be_cleared() {
        let mut pool = ProxyPool::default();
        pool.import(&["http://proxy.example.test:8080".into()], 1)
            .unwrap();
        let id = pool.summary().entries[0].id.clone();
        pool.assign_id(&id, "synthetic-account").unwrap();
        let before = pool.summary();
        pool.rename(&id, "  Рабочий прокси  ").unwrap();
        let named = pool.summary();
        assert_eq!(named.entries[0].label.as_deref(), Some("Рабочий прокси"));
        assert_eq!(named.entries[0].endpoint, before.entries[0].endpoint);
        assert_eq!(
            named.entries[0].assigned_account_ids,
            before.entries[0].assigned_account_ids
        );
        assert_eq!(named.entries[0].id, id);
        assert_eq!(named.entries[0].created_at_ms, 1);
        let restored = ProxyPool::from_json(&serde_json::to_string(&pool).unwrap()).unwrap();
        assert_eq!(restored.summary(), named);
        for invalid in ["x".repeat(81), "line\nbreak".into(), "control\0name".into()] {
            assert!(pool.rename(&id, &invalid).is_err());
            assert_eq!(pool.summary(), named);
        }
        assert!(pool.rename("missing", "name").is_err());
        pool.rename(&id, " ").unwrap();
        assert_eq!(pool.summary(), before);
        let document = serde_json::to_value(&pool).unwrap();
        assert!(document["entries"][0].get("label").is_none());
        assert_eq!(
            ProxyPool::from_json(&document.to_string())
                .unwrap()
                .summary(),
            before
        );
    }

    #[test]
    fn source_proxy_resolves_only_a_stored_valid_proxy() {
        let mut pool = ProxyPool::default();
        pool.import(&["http://user:secret@host.example:8080".to_string()], 1)
            .unwrap();
        let proxy_id = pool.summary().entries[0].id.clone();

        assert_eq!(pool.source_url(None).unwrap(), None);
        assert_eq!(
            pool.source_url(Some(&proxy_id)).unwrap().as_deref(),
            Some("http://user:secret@host.example:8080/")
        );
        assert!(
            pool.source_url(Some("proxy_missing")).is_err(),
            "a missing proxy must not fall back to a direct connection"
        );
    }

    #[test]
    fn stored_proxy_is_found_by_its_normalized_url() {
        let mut pool = ProxyPool::default();
        pool.import(&["http://user:secret@host.example:8080".to_string()], 1)
            .unwrap();
        let proxy_id = pool.summary().entries[0].id.clone();
        let stored_url = pool.stored_url(&proxy_id).unwrap();

        assert_eq!(pool.id_for_url(&stored_url), Some(proxy_id.clone()));
        assert_eq!(
            pool.id_for_url("http://user:secret@host.example:8080"),
            Some(proxy_id)
        );
        assert_eq!(pool.id_for_url("http://other.example:8080/"), None);
    }

    #[test]
    fn account_proxy_identity_follows_credentials_including_inherited_and_direct_routes() {
        let mut pool = ProxyPool::default();
        pool.import(
            &[
                "http://old.example:8080".into(),
                "http://new.example:8080".into(),
            ],
            1,
        )
        .unwrap();
        let old_id = pool.summary().entries[0].id.clone();
        let new_id = pool.summary().entries[1].id.clone();
        pool.assign_id(&old_id, "account").unwrap();
        let inherited = credentials_without_proxy();
        let explicit = inherited
            .clone()
            .with_proxy_url(Some("http://new.example:8080".into()))
            .unwrap();
        assert_eq!(pool.proxy_id_for_account("account"), Some(old_id.clone()));
        assert_eq!(
            account_proxy_pool_id(&pool, &explicit, Some(&old_id)),
            Some(new_id)
        );
        assert_eq!(
            account_proxy_pool_id(&pool, &inherited, Some(&old_id)),
            Some(old_id.clone())
        );
        assert_eq!(account_proxy_pool_id(&pool, &inherited, None), None);
        let direct = inherited.with_proxy_route(None, true).unwrap();
        assert_eq!(account_proxy_pool_id(&pool, &direct, Some(&old_id)), None);
    }

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
        let proxy_inputs = vec![
            "host.example:8080:user:secret".to_string(),
            "http://user:secret@host.example:8080".to_string(),
        ];
        assert_eq!(pool.import(&proxy_inputs, 1).unwrap(), (1, 1));

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

    #[test]
    fn restored_proxy_timezones_are_normalized_and_validated() {
        let document = serde_json::json!({
            "version": PROXY_POOL_VERSION,
            "entries": [{
                "id": "proxy_test",
                "url": "http://host.example:8080/",
                "assignedAccountIds": [],
                "createdAtMs": 1,
                "timezone": " Etc/UTC "
                ,"timezoneObservedAtMs": 100
            }]
        });
        let pool = ProxyPool::from_json(&document.to_string()).unwrap();
        assert_eq!(
            pool.timezone_observation("proxy_test")
                .unwrap()
                .as_ref()
                .map(|evidence| evidence.timezone.as_str()),
            Some("Etc/UTC")
        );
        let restored = ProxyPool::from_json(&serde_json::to_string(&pool).unwrap()).unwrap();
        assert_eq!(restored.summary(), pool.summary());

        let mut document = document;
        document["entries"][0]["timezone"] = serde_json::json!(" ");
        assert!(ProxyPool::from_json(&document.to_string())
            .unwrap()
            .timezone_observation("proxy_test")
            .unwrap()
            .is_none());
        document["entries"][0]["timezone"] = serde_json::json!("Invalid/Timezone");
        assert!(ProxyPool::from_json(&document.to_string()).is_err());
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

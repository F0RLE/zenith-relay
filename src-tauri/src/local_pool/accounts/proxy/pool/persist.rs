use super::super::{MAX_PROXY_POOL_ENTRIES, PROXY_POOL_VERSION};
use super::{PersistedProxyPool, ProxyPool, StoredProxy, PROXY_POOL_SECRET_REF};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    store::secret_store,
};
use std::collections::HashSet;

impl ProxyPool {
    pub(crate) fn load() -> Result<Self> {
        let Some(content) = secret_store::load(PROXY_POOL_SECRET_REF)? else {
            return Ok(Self::default());
        };
        Self::from_json(&content)
    }

    pub(in crate::local_pool::accounts::proxy) fn from_json(content: &str) -> Result<Self> {
        let persisted: PersistedProxyPool = serde_json::from_str(content).map_err(|_| {
            LocalPoolError::new(ErrorCode::RecoveryRequired, "stored proxy pool is invalid")
        })?;
        if !matches!(persisted.version, 1 | PROXY_POOL_VERSION) {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "stored proxy pool has unsupported metadata",
            ));
        }
        let pool = Self {
            version: PROXY_POOL_VERSION,
            entries: persisted
                .entries
                .into_iter()
                .map(|stored_proxy| {
                    let mut assigned_account_ids = stored_proxy.assigned_account_ids;
                    if let Some(account_id) = stored_proxy.assigned_account_id {
                        assigned_account_ids.push(account_id);
                    }
                    StoredProxy {
                        id: stored_proxy.id,
                        url: stored_proxy.url,
                        assigned_account_ids,
                        created_at_ms: stored_proxy.created_at_ms,
                    }
                })
                .collect(),
        };
        pool.validate()?;
        Ok(pool)
    }

    pub(crate) fn save(&self) -> Result<()> {
        self.validate()?;
        if self.entries.is_empty() {
            return secret_store::delete(PROXY_POOL_SECRET_REF);
        }
        let content = serde_json::to_string(self).map_err(|_| {
            LocalPoolError::new(ErrorCode::InvalidState, "proxy pool serialization failed")
        })?;
        secret_store::save(PROXY_POOL_SECRET_REF, &content)
    }

    fn validate(&self) -> Result<()> {
        if self.version != PROXY_POOL_VERSION || self.entries.len() > MAX_PROXY_POOL_ENTRIES {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "stored proxy pool has unsupported metadata",
            ));
        }
        let mut proxy_ids = HashSet::new();
        let mut urls = HashSet::new();
        let mut accounts = HashSet::new();
        for stored_proxy in &self.entries {
            let valid = !stored_proxy.id.is_empty()
                && proxy_ids.insert(stored_proxy.id.as_str())
                && zenith_relay_core::normalize_proxy_url(&stored_proxy.url)
                    .is_ok_and(|url| url == stored_proxy.url)
                && urls.insert(stored_proxy.url.as_str())
                && stored_proxy.assigned_account_ids.iter().all(|account_id| {
                    !account_id.is_empty() && accounts.insert(account_id.as_str())
                });
            if !valid {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "stored proxy pool contains invalid or duplicate entries",
                ));
            }
        }
        Ok(())
    }
}

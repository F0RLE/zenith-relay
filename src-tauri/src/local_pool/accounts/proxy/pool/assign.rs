use super::super::MAX_PROXY_POOL_ENTRIES;
use super::{ProxyPool, StoredProxy};
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use std::collections::{HashMap, HashSet};

impl ProxyPool {
    pub(crate) fn import(&mut self, proxy_urls: &[String], now_ms: u64) -> Result<(usize, usize)> {
        if proxy_urls.is_empty() || proxy_urls.len() > MAX_PROXY_POOL_ENTRIES {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "proxy import must contain between 1 and 1000 entries",
            ));
        }
        let mut known = self
            .entries
            .iter()
            .map(|stored_proxy| stored_proxy.url.clone())
            .collect::<HashSet<_>>();
        let mut added = 0;
        let mut duplicates = 0;
        for proxy_url_text in proxy_urls {
            let url = zenith_relay_core::normalize_proxy_url(proxy_url_text)
                .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
            if !known.insert(url.clone()) {
                duplicates += 1;
                continue;
            }
            if self.entries.len() >= MAX_PROXY_POOL_ENTRIES {
                return Err(LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "proxy pool limit of 1000 entries is reached",
                ));
            }
            self.entries.push(StoredProxy {
                id: format!("proxy_{}", uuid::Uuid::new_v4().simple()),
                url,
                assigned_account_ids: Vec::new(),
                created_at_ms: now_ms,
                last_check: None,
            });
            added += 1;
        }
        Ok((added, duplicates))
    }

    pub(crate) fn reconcile(
        &mut self,
        account_proxies: &[(String, Option<String>)],
        now_ms: u64,
    ) -> bool {
        let previous_entries = self.entries.clone();
        let account_proxy_by_id = account_proxies
            .iter()
            .map(|(account_id, proxy)| (account_id.as_str(), proxy.as_deref()))
            .collect::<HashMap<_, _>>();
        for stored_proxy in &mut self.entries {
            stored_proxy.assigned_account_ids.retain(|account_id| {
                account_proxy_by_id
                    .get(account_id.as_str())
                    .copied()
                    .flatten()
                    == Some(stored_proxy.url.as_str())
            });
        }
        for (account_id, proxy_url) in account_proxies {
            let Some(proxy_url) = proxy_url else { continue };
            if self.entries.iter().any(|stored_proxy| {
                stored_proxy
                    .assigned_account_ids
                    .iter()
                    .any(|assigned_account_id| assigned_account_id == account_id)
            }) {
                continue;
            }
            if let Some(stored_proxy) = self
                .entries
                .iter_mut()
                .find(|stored_proxy| stored_proxy.url == *proxy_url)
            {
                stored_proxy.assigned_account_ids.push(account_id.clone());
            } else if self.entries.len() < MAX_PROXY_POOL_ENTRIES {
                self.entries.push(StoredProxy {
                    id: format!("proxy_{}", uuid::Uuid::new_v4().simple()),
                    url: proxy_url.clone(),
                    assigned_account_ids: vec![account_id.clone()],
                    created_at_ms: now_ms,
                    last_check: None,
                });
            }
        }
        self.entries != previous_entries
    }

    pub(crate) fn assign_automatic(&mut self, account_id: &str) -> Option<String> {
        if let Some(stored_proxy) = self.entries.iter().find(|stored_proxy| {
            stored_proxy
                .assigned_account_ids
                .iter()
                .any(|assigned_account_id| assigned_account_id == account_id)
        }) {
            return Some(stored_proxy.url.clone());
        }
        let index = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, stored_proxy)| stored_proxy.assigned_account_ids.len())?
            .0;
        self.assign_index(index, account_id)
    }

    pub(crate) fn assign_id(&mut self, proxy_id: &str, account_id: &str) -> Result<String> {
        let index = self
            .entries
            .iter()
            .position(|stored_proxy| stored_proxy.id == proxy_id)
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "stored proxy not found"))?;
        self.assign_index(index, account_id)
            .ok_or_else(|| LocalPoolError::new(ErrorCode::InvalidState, "stored proxy is invalid"))
    }

    pub(crate) fn assign_url(
        &mut self,
        proxy_url_text: &str,
        account_id: &str,
        now_ms: u64,
    ) -> Result<String> {
        let url = zenith_relay_core::normalize_proxy_url(proxy_url_text)
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        let index = match self
            .entries
            .iter()
            .position(|stored_proxy| stored_proxy.url == url)
        {
            Some(index) => index,
            None if self.entries.len() < MAX_PROXY_POOL_ENTRIES => {
                self.entries.push(StoredProxy {
                    id: format!("proxy_{}", uuid::Uuid::new_v4().simple()),
                    url,
                    assigned_account_ids: Vec::new(),
                    created_at_ms: now_ms,
                    last_check: None,
                });
                self.entries.len() - 1
            }
            None => {
                return Err(LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "proxy pool limit of 1000 entries is reached",
                ))
            }
        };
        Ok(self
            .assign_index(index, account_id)
            .expect("stored proxy URL was validated"))
    }

    pub(crate) fn release(&mut self, account_id: &str) {
        for stored_proxy in &mut self.entries {
            stored_proxy
                .assigned_account_ids
                .retain(|assigned_account_id| assigned_account_id != account_id);
        }
    }

    pub(crate) fn delete(&mut self, proxy_id: &str) -> Result<()> {
        self.delete_many(&[proxy_id.to_string()])
    }

    pub(crate) fn delete_many(&mut self, proxy_ids: &[String]) -> Result<()> {
        let proxy_id_set = proxy_ids.iter().map(String::as_str).collect::<HashSet<_>>();
        if proxy_id_set.len() != proxy_ids.len()
            || proxy_id_set.iter().any(|proxy_id| {
                !self
                    .entries
                    .iter()
                    .any(|stored_proxy| stored_proxy.id == *proxy_id)
            })
        {
            return Err(LocalPoolError::new(
                ErrorCode::NotFound,
                "stored proxy not found",
            ));
        }
        if self.entries.iter().any(|stored_proxy| {
            proxy_id_set.contains(stored_proxy.id.as_str())
                && !stored_proxy.assigned_account_ids.is_empty()
        }) {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "release the proxy from its accounts before deleting it",
            ));
        }
        self.entries
            .retain(|stored_proxy| !proxy_id_set.contains(stored_proxy.id.as_str()));
        Ok(())
    }

    fn assign_index(&mut self, index: usize, account_id: &str) -> Option<String> {
        let url = self.entries.get(index)?.url.clone();
        self.release(account_id);
        self.entries[index]
            .assigned_account_ids
            .push(account_id.to_string());
        Some(url)
    }
}

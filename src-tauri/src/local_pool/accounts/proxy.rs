use super::{
    authority::CodexRefreshClient,
    credentials::{CredentialRefresh, StoredCodexCredentials},
    oauth::CodexOAuthClient,
};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::GatewaySettings,
    store::secret_store,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
};
use url::Url;
use zenith_relay_core::{
    accounts::{TokenRefreshFailure, TokenRefreshFailureKind},
    protocol::ProxyMode,
    ProxyConfig,
};

pub const COMMON_PROXY_SECRET_REF: &str = "proxy:common";
pub const PROXY_POOL_SECRET_REF: &str = "proxy:pool";
const PROXY_POOL_VERSION: u32 = 2;
const MAX_PROXY_POOL_ENTRIES: usize = 1_000;

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProxyPool {
    version: u32,
    entries: Vec<StoredProxy>,
}

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredProxy {
    id: String,
    url: String,
    assigned_account_ids: Vec<String>,
    created_at_ms: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedProxyPool {
    version: u32,
    entries: Vec<PersistedStoredProxy>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PersistedStoredProxy {
    id: String,
    url: String,
    #[serde(default)]
    assigned_account_ids: Vec<String>,
    #[serde(default)]
    assigned_account_id: Option<String>,
    created_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyPoolEntrySummary {
    pub id: String,
    pub endpoint: String,
    pub assigned_account_ids: Vec<String>,
    pub country_code: Option<String>,
    pub region: Option<String>,
    pub created_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyPoolSummary {
    pub entries: Vec<ProxyPoolEntrySummary>,
    pub total: usize,
    pub free: usize,
    pub assigned: usize,
}

impl Default for ProxyPool {
    fn default() -> Self {
        Self {
            version: PROXY_POOL_VERSION,
            entries: Vec::new(),
        }
    }
}

impl ProxyPool {
    pub(crate) fn load() -> Result<Self> {
        let Some(content) = secret_store::load(PROXY_POOL_SECRET_REF)? else {
            return Ok(Self::default());
        };
        Self::from_json(&content)
    }

    fn from_json(content: &str) -> Result<Self> {
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
                .map(|entry| {
                    let mut assigned_account_ids = entry.assigned_account_ids;
                    if let Some(account_id) = entry.assigned_account_id {
                        assigned_account_ids.push(account_id);
                    }
                    StoredProxy {
                        id: entry.id,
                        url: entry.url,
                        assigned_account_ids,
                        created_at_ms: entry.created_at_ms,
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

    pub(crate) fn import(&mut self, values: &[String], now_ms: u64) -> Result<(usize, usize)> {
        if values.is_empty() || values.len() > MAX_PROXY_POOL_ENTRIES {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "proxy import must contain between 1 and 1000 entries",
            ));
        }
        let mut known = self
            .entries
            .iter()
            .map(|entry| entry.url.clone())
            .collect::<HashSet<_>>();
        let mut added = 0;
        let mut duplicates = 0;
        for value in values {
            let url = zenith_relay_core::normalize_proxy_url(value)
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
        let before = self.entries.clone();
        let current = account_proxies
            .iter()
            .map(|(account_id, proxy)| (account_id.as_str(), proxy.as_deref()))
            .collect::<HashMap<_, _>>();
        for entry in &mut self.entries {
            entry.assigned_account_ids.retain(|account_id| {
                current.get(account_id.as_str()).copied().flatten() == Some(entry.url.as_str())
            });
        }
        for (account_id, proxy_url) in account_proxies {
            let Some(proxy_url) = proxy_url else { continue };
            if self
                .entries
                .iter()
                .any(|entry| entry.assigned_account_ids.iter().any(|id| id == account_id))
            {
                continue;
            }
            if let Some(entry) = self
                .entries
                .iter_mut()
                .find(|entry| entry.url == *proxy_url)
            {
                entry.assigned_account_ids.push(account_id.clone());
            } else if self.entries.len() < MAX_PROXY_POOL_ENTRIES {
                self.entries.push(StoredProxy {
                    id: format!("proxy_{}", uuid::Uuid::new_v4().simple()),
                    url: proxy_url.clone(),
                    assigned_account_ids: vec![account_id.clone()],
                    created_at_ms: now_ms,
                });
            }
        }
        self.entries != before
    }

    pub(crate) fn assign_automatic(&mut self, account_id: &str) -> Option<String> {
        if let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.assigned_account_ids.iter().any(|id| id == account_id))
        {
            return Some(entry.url.clone());
        }
        let index = self
            .entries
            .iter()
            .enumerate()
            .min_by_key(|(_, entry)| entry.assigned_account_ids.len())?
            .0;
        self.assign_index(index, account_id)
    }

    pub(crate) fn assign_id(&mut self, proxy_id: &str, account_id: &str) -> Result<String> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.id == proxy_id)
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "stored proxy not found"))?;
        self.assign_index(index, account_id)
            .ok_or_else(|| LocalPoolError::new(ErrorCode::InvalidState, "stored proxy is invalid"))
    }

    pub(crate) fn assign_url(
        &mut self,
        value: &str,
        account_id: &str,
        now_ms: u64,
    ) -> Result<String> {
        let url = zenith_relay_core::normalize_proxy_url(value)
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        let index = match self.entries.iter().position(|entry| entry.url == url) {
            Some(index) => index,
            None if self.entries.len() < MAX_PROXY_POOL_ENTRIES => {
                self.entries.push(StoredProxy {
                    id: format!("proxy_{}", uuid::Uuid::new_v4().simple()),
                    url,
                    assigned_account_ids: Vec::new(),
                    created_at_ms: now_ms,
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
        for entry in &mut self.entries {
            entry.assigned_account_ids.retain(|id| id != account_id);
        }
    }

    pub(crate) fn delete(&mut self, proxy_id: &str) -> Result<()> {
        self.delete_many(&[proxy_id.to_string()])
    }

    pub(crate) fn delete_many(&mut self, proxy_ids: &[String]) -> Result<()> {
        let ids = proxy_ids.iter().map(String::as_str).collect::<HashSet<_>>();
        if ids.len() != proxy_ids.len()
            || ids
                .iter()
                .any(|id| !self.entries.iter().any(|entry| entry.id == *id))
        {
            return Err(LocalPoolError::new(
                ErrorCode::NotFound,
                "stored proxy not found",
            ));
        }
        if self
            .entries
            .iter()
            .any(|entry| ids.contains(entry.id.as_str()) && !entry.assigned_account_ids.is_empty())
        {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "release the proxy from its accounts before deleting it",
            ));
        }
        self.entries
            .retain(|entry| !ids.contains(entry.id.as_str()));
        Ok(())
    }

    pub(crate) fn assigned_account_ids(&self, proxy_id: &str) -> Result<Vec<String>> {
        self.entries
            .iter()
            .find(|entry| entry.id == proxy_id)
            .map(|entry| entry.assigned_account_ids.clone())
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "stored proxy not found"))
    }

    pub(crate) fn summary(&self) -> ProxyPoolSummary {
        let entries = self
            .entries
            .iter()
            .map(|entry| {
                let (country_code, region) = declared_proxy_location(&entry.url);
                ProxyPoolEntrySummary {
                    id: entry.id.clone(),
                    endpoint: proxy_endpoint(&entry.url),
                    assigned_account_ids: entry.assigned_account_ids.clone(),
                    country_code,
                    region,
                    created_at_ms: entry.created_at_ms,
                }
            })
            .collect::<Vec<_>>();
        let free = entries
            .iter()
            .filter(|entry| entry.assigned_account_ids.is_empty())
            .count();
        ProxyPoolSummary {
            total: entries.len(),
            assigned: entries.len() - free,
            free,
            entries,
        }
    }

    fn assign_index(&mut self, index: usize, account_id: &str) -> Option<String> {
        let url = self.entries.get(index)?.url.clone();
        self.release(account_id);
        self.entries[index]
            .assigned_account_ids
            .push(account_id.to_string());
        Some(url)
    }

    fn validate(&self) -> Result<()> {
        if self.version != PROXY_POOL_VERSION || self.entries.len() > MAX_PROXY_POOL_ENTRIES {
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "stored proxy pool has unsupported metadata",
            ));
        }
        let mut ids = HashSet::new();
        let mut urls = HashSet::new();
        let mut accounts = HashSet::new();
        for entry in &self.entries {
            let valid = !entry.id.is_empty()
                && ids.insert(entry.id.as_str())
                && zenith_relay_core::normalize_proxy_url(&entry.url)
                    .is_ok_and(|url| url == entry.url)
                && urls.insert(entry.url.as_str())
                && entry.assigned_account_ids.iter().all(|account_id| {
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

fn proxy_endpoint(value: &str) -> String {
    let Ok(url) = Url::parse(value) else {
        return "invalid".to_string();
    };
    let host = url.host_str().unwrap_or("invalid");
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    format!(
        "{}://{}:{}",
        url.scheme(),
        host,
        url.port_or_known_default().unwrap_or_default()
    )
}

fn declared_proxy_location(value: &str) -> (Option<String>, Option<String>) {
    let Ok(url) = Url::parse(value) else {
        return (None, None);
    };
    let username = url::form_urlencoded::parse(url.username().as_bytes())
        .next()
        .map(|(value, _)| value.into_owned())
        .unwrap_or_default();
    let country = selector_value(
        &username,
        &[
            "__cr.",
            ";cr.",
            "__country.",
            ";country.",
            "_country-",
            "-country-",
        ],
    )
    .filter(|value| {
        value.len() == 2
            && value
                .chars()
                .all(|character| character.is_ascii_alphabetic())
    })
    .map(|value| value.to_ascii_uppercase());
    let region = selector_value(
        &username,
        &[
            "__region.",
            ";region.",
            "__state.",
            ";state.",
            "_region-",
            "-region-",
            "_state-",
            "-state-",
        ],
    );
    (country, region)
}

fn selector_value(value: &str, markers: &[&str]) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    markers.iter().find_map(|marker| {
        let start = lower.find(marker)? + marker.len();
        let selection = value[start..]
            .chars()
            .take_while(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
            .take(32)
            .collect::<String>();
        (!selection.is_empty()).then_some(selection)
    })
}

pub fn effective_proxy_url(
    settings: &GatewaySettings,
    credentials: &StoredCodexCredentials,
) -> Result<Option<String>> {
    let proxy = choose_proxy_url(
        credentials.proxy_url(),
        settings.common_proxy_configured,
        || secret_store::load(COMMON_PROXY_SECRET_REF),
    )?;
    ensure_account_proxy(settings, proxy.as_ref().map(|_| ()))?;
    Ok(proxy)
}

pub fn effective_proxy_config(
    settings: &GatewaySettings,
    credentials: &StoredCodexCredentials,
) -> Result<Option<ProxyConfig>> {
    effective_proxy_url(settings, credentials)?
        .map(|value| {
            ProxyConfig::parse(&value).map_err(|_| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "stored account proxy URL is invalid",
                )
            })
        })
        .transpose()
}

pub fn common_proxy_config(settings: &GatewaySettings) -> Result<Option<ProxyConfig>> {
    if !settings.common_proxy_configured {
        return Ok(None);
    }
    let value = secret_store::load(COMMON_PROXY_SECRET_REF)?.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::SecretStoreUnavailable,
            "common account proxy is configured but its secret is unavailable",
        )
    })?;
    ProxyConfig::parse(&value).map(Some).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "stored common proxy URL is invalid",
        )
    })
}

pub fn proxy_status(
    settings: &GatewaySettings,
    credentials: &StoredCodexCredentials,
    common_available: bool,
) -> (ProxyMode, bool) {
    if credentials.proxy_url().is_some() {
        return (
            ProxyMode::Account,
            credentials
                .proxy_url()
                .is_some_and(|value| ProxyConfig::parse(value).is_ok()),
        );
    }
    if settings.common_proxy_configured {
        return (ProxyMode::Common, common_available);
    }
    (ProxyMode::Direct, !settings.account_proxy_required)
}

pub fn ensure_account_proxy<T>(settings: &GatewaySettings, proxy: Option<T>) -> Result<()> {
    if settings.account_proxy_required && proxy.is_none() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "an account proxy is required; direct account traffic is blocked",
        ));
    }
    Ok(())
}

pub fn common_proxy_available(settings: &GatewaySettings) -> bool {
    settings.common_proxy_configured
        && secret_store::load(COMMON_PROXY_SECRET_REF)
            .ok()
            .flatten()
            .is_some_and(|value| ProxyConfig::parse(&value).is_ok())
}

fn choose_proxy_url(
    account_proxy: Option<&str>,
    common_configured: bool,
    load_common: impl FnOnce() -> Result<Option<String>>,
) -> Result<Option<String>> {
    if let Some(value) = account_proxy {
        return Ok(Some(value.to_string()));
    }
    if !common_configured {
        return Ok(None);
    }
    load_common()?.map(Some).ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::SecretStoreUnavailable,
            "common account proxy is configured but its secret is unavailable",
        )
    })
}

pub struct ProxyRefreshClient {
    direct: CodexOAuthClient,
    direct_accounts: HashSet<String>,
    clients: HashMap<String, CodexOAuthClient>,
}

impl ProxyRefreshClient {
    pub fn new(proxies: impl IntoIterator<Item = (String, Option<ProxyConfig>)>) -> Result<Self> {
        let direct = CodexOAuthClient::new_with_proxy(None).map_err(|_| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "failed to initialize account refresh client",
            )
        })?;
        let mut direct_accounts = HashSet::new();
        let mut clients = HashMap::new();
        for (account_id, proxy) in proxies {
            match proxy {
                Some(proxy) => {
                    let client = CodexOAuthClient::new_with_proxy(Some(&proxy)).map_err(|_| {
                        LocalPoolError::new(
                            ErrorCode::InvalidState,
                            "failed to initialize account proxy client",
                        )
                    })?;
                    clients.insert(account_id, client);
                }
                None => {
                    direct_accounts.insert(account_id);
                }
            }
        }
        Ok(Self {
            direct,
            direct_accounts,
            clients,
        })
    }
}

impl CodexRefreshClient for ProxyRefreshClient {
    fn refresh<'a>(
        &'a self,
        local_account_id: &'a str,
        _provider_account_id: Option<&'a str>,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> Pin<
        Box<
            dyn Future<Output = std::result::Result<CredentialRefresh, TokenRefreshFailure>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let client = match self.clients.get(local_account_id) {
                Some(client) => client,
                None if self.direct_accounts.contains(local_account_id) => &self.direct,
                None => {
                    return Err(TokenRefreshFailure::new(
                        TokenRefreshFailureKind::Transient,
                        "proxy_client_missing",
                    ))
                }
            };
            let tokens = client.exchange_refresh_token(refresh_token, now_ms).await?;
            CredentialRefresh::from_oauth(tokens).map_err(|_| {
                TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "invalid_refresh_response",
                )
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_proxy_overrides_common_and_missing_common_fails_closed() {
        let account = choose_proxy_url(Some("http://account.example:8080/"), true, || {
            panic!("common proxy must not be loaded for an account override")
        })
        .unwrap();
        assert_eq!(account.as_deref(), Some("http://account.example:8080/"));

        let error = choose_proxy_url(None, true, || Ok(None)).unwrap_err();
        assert!(matches!(error.code, ErrorCode::SecretStoreUnavailable));
        assert_eq!(choose_proxy_url(None, false, || Ok(None)).unwrap(), None);
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

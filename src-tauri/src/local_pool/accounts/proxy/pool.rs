use super::{PROXY_POOL_SECRET_REF, PROXY_POOL_VERSION};
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use serde::{Deserialize, Serialize};
use url::Url;
use zenith_relay_core::ProxyConfig;

mod assign;
mod persist;

#[derive(Clone, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProxyPool {
    pub(super) version: u32,
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
    pub(crate) fn assigned_account_ids(&self, proxy_id: &str) -> Result<Vec<String>> {
        self.entries
            .iter()
            .find(|stored_proxy| stored_proxy.id == proxy_id)
            .map(|stored_proxy| stored_proxy.assigned_account_ids.clone())
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "stored proxy not found"))
    }

    pub(crate) fn config(&self, proxy_id: &str) -> Result<ProxyConfig> {
        let stored_proxy = self
            .entries
            .iter()
            .find(|stored_proxy| stored_proxy.id == proxy_id)
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "stored proxy not found"))?;
        ProxyConfig::parse(&stored_proxy.url)
            .map_err(|_| LocalPoolError::new(ErrorCode::InvalidState, "stored proxy is invalid"))
    }

    pub(crate) fn stored_url(&self, proxy_id: &str) -> Result<String> {
        self.entries
            .iter()
            .find(|stored_proxy| stored_proxy.id == proxy_id)
            .map(|stored_proxy| stored_proxy.url.clone())
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "stored proxy not found"))
    }

    pub(crate) fn summary(&self) -> ProxyPoolSummary {
        let proxy_summaries = self
            .entries
            .iter()
            .map(|stored_proxy| {
                let (country_code, region) = declared_proxy_location(&stored_proxy.url);
                ProxyPoolEntrySummary {
                    id: stored_proxy.id.clone(),
                    endpoint: proxy_endpoint(&stored_proxy.url),
                    assigned_account_ids: stored_proxy.assigned_account_ids.clone(),
                    country_code,
                    region,
                    created_at_ms: stored_proxy.created_at_ms,
                }
            })
            .collect::<Vec<_>>();
        let free = proxy_summaries
            .iter()
            .filter(|proxy| proxy.assigned_account_ids.is_empty())
            .count();
        ProxyPoolSummary {
            total: proxy_summaries.len(),
            assigned: proxy_summaries.len() - free,
            free,
            entries: proxy_summaries,
        }
    }
}

fn proxy_endpoint(proxy_url: &str) -> String {
    let Ok(url) = Url::parse(proxy_url) else {
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

fn declared_proxy_location(proxy_url: &str) -> (Option<String>, Option<String>) {
    let Ok(url) = Url::parse(proxy_url) else {
        return (None, None);
    };
    let username = url::form_urlencoded::parse(url.username().as_bytes())
        .next()
        .map(|(username, _)| username.into_owned())
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
    .filter(|country_code| {
        country_code.len() == 2
            && country_code
                .chars()
                .all(|character| character.is_ascii_alphabetic())
    })
    .map(|country_code| country_code.to_ascii_uppercase());
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

fn selector_value(username: &str, markers: &[&str]) -> Option<String> {
    let lower = username.to_ascii_lowercase();
    markers.iter().find_map(|marker| {
        let start = lower.find(marker)? + marker.len();
        let selection = username[start..]
            .chars()
            .take_while(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
            })
            .take(32)
            .collect::<String>();
        (!selection.is_empty()).then_some(selection)
    })
}

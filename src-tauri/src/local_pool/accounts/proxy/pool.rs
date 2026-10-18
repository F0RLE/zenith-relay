use super::check::ProxyCheckResult;
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
    #[serde(skip_serializing_if = "Option::is_none")]
    timezone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_check: Option<ProxyCheckResult>,
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
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    last_check: Option<ProxyCheckResult>,
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
    pub timezone: Option<String>,
    pub last_check: Option<ProxyCheckResult>,
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
    /// A deleted/replaced proxy or an older completion cannot overwrite a newer check.
    pub(crate) fn record_check(&mut self, expected_url: &str, result: ProxyCheckResult) -> bool {
        let Some(stored_proxy) = self.entries.iter_mut().find(|stored_proxy| {
            stored_proxy.id == result.proxy_id && stored_proxy.url == expected_url
        }) else {
            return false;
        };
        if stored_proxy
            .last_check
            .as_ref()
            .is_some_and(|previous| previous.checked_at_ms >= result.checked_at_ms)
        {
            return false;
        }
        let mut result = result;
        result.timezone = result.timezone.take().and_then(|timezone| {
            zenith_relay_core::normalize_timezone(Some(timezone))
                .ok()
                .flatten()
        });
        if result.error_code.is_some() || result.ip.is_none() {
            result.timezone = None;
        }
        if result.error_code.is_none() && result.ip.is_some() {
            if let Some(timezone) = &result.timezone {
                stored_proxy.timezone = Some(timezone.clone());
            } else if stored_proxy.last_check.as_ref().and_then(|check| check.ip) != result.ip {
                // An observed exit change invalidates its old zone. A failed
                // geolocation must not assign a guessed or stale timezone.
                stored_proxy.timezone = None;
            }
        }
        stored_proxy.last_check = Some(result);
        true
    }

    pub(crate) fn assigned_account_ids(&self, proxy_id: &str) -> Result<Vec<String>> {
        self.entries
            .iter()
            .find(|stored_proxy| stored_proxy.id == proxy_id)
            .map(|stored_proxy| stored_proxy.assigned_account_ids.clone())
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "stored proxy not found"))
    }

    pub(crate) fn proxy_id_for_account(&self, account_id: &str) -> Option<String> {
        self.entries
            .iter()
            .find(|stored_proxy| {
                stored_proxy
                    .assigned_account_ids
                    .iter()
                    .any(|assigned_account_id| assigned_account_id == account_id)
            })
            .map(|stored_proxy| stored_proxy.id.clone())
    }

    pub(crate) fn id_for_url(&self, proxy_url: &str) -> Option<String> {
        let normalized_url = zenith_relay_core::normalize_proxy_url(proxy_url).ok()?;
        self.entries
            .iter()
            .find(|stored_proxy| {
                zenith_relay_core::normalize_proxy_url(&stored_proxy.url)
                    .ok()
                    .as_deref()
                    == Some(normalized_url.as_str())
            })
            .map(|stored_proxy| stored_proxy.id.clone())
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

    /// Resolves the proxy URL of an API source. A source without a proxy uses
    /// no proxy; a source whose proxy is missing or invalid is an error and
    /// must never fall back to a direct connection.
    pub(crate) fn source_url(&self, proxy_id: Option<&str>) -> Result<Option<String>> {
        let Some(proxy_id) = proxy_id else {
            return Ok(None);
        };
        let proxy_url = self.stored_url(proxy_id).map_err(|_| {
            LocalPoolError::new(ErrorCode::InvalidState, "source proxy is not available")
        })?;
        zenith_relay_core::normalize_proxy_url(&proxy_url)
            .map(Some)
            .map_err(|_| LocalPoolError::new(ErrorCode::InvalidState, "source proxy is invalid"))
    }

    pub(crate) fn timezone(&self, proxy_id: &str) -> Result<Option<String>> {
        self.entries
            .iter()
            .find(|stored_proxy| stored_proxy.id == proxy_id)
            .map(|stored_proxy| stored_proxy.timezone.clone())
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "stored proxy not found"))
    }

    pub(crate) fn set_timezone(
        &mut self,
        proxy_id: &str,
        timezone: Option<String>,
    ) -> Result<bool> {
        let timezone = zenith_relay_core::normalize_timezone(timezone)
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        let stored_proxy = self
            .entries
            .iter_mut()
            .find(|stored_proxy| stored_proxy.id == proxy_id)
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "stored proxy not found"))?;
        if stored_proxy.timezone == timezone {
            return Ok(false);
        }
        stored_proxy.timezone = timezone;
        Ok(true)
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
                    timezone: stored_proxy.timezone.clone(),
                    last_check: stored_proxy.last_check.clone(),
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

#[cfg(test)]
mod checks;

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

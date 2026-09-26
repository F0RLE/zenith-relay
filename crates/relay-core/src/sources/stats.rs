mod formats;
#[cfg(test)]
mod tests;
mod transport;

use crate::scheduler::refresh::http::ManagementHttpScope;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use transport::{StatsClient, StatsResult};
use url::Url;

#[cfg(test)]
pub(super) use formats::{openrouter_stats, zenith_stats};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatsProvider {
    Zenith,
    #[serde(rename = "openrouter")]
    OpenRouter,
    #[serde(rename = "sub2api")]
    Sub2Api,
    NewApi,
    Billing,
    Deepseek,
    #[serde(rename = "siliconflow")]
    SiliconFlow,
    Unsupported,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatsStatus {
    #[default]
    Available,
    Unsupported,
    Unauthorized,
    RateLimited,
    Unavailable,
    InvalidResponse,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceBalanceKind {
    #[default]
    Wallet,
    KeyQuota,
    Subscription,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum SourceStatsCurrency {
    Usd,
    Cny,
    Credits,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceStatsAmount {
    pub currency: SourceStatsCurrency,
    pub balance_micros: Option<i64>,
    pub spent_micros: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceProviderStats {
    pub provider: SourceStatsProvider,
    pub balance_micro_usd: Option<i64>,
    pub spent_micro_usd: Option<i64>,
    pub requests: Option<u64>,
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub status: SourceStatsStatus,
    #[serde(default)]
    pub balance_kind: SourceBalanceKind,
    #[serde(default)]
    pub balance_unlimited: bool,
    #[serde(default)]
    pub amounts: Vec<SourceStatsAmount>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub as_of_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_error: Option<SourceStatsStatus>,
}

fn is_false(value: &bool) -> bool {
    !value
}

impl SourceProviderStats {
    /// Preserve a successful value only inside the same fenced source scope.
    pub fn observed(mut self, previous: Option<&Self>, now_ms: u64) -> Self {
        if self.status == SourceStatsStatus::Available {
            self.as_of_ms = Some(now_ms);
            self.stale = false;
            self.refresh_error = None;
        } else if self.status != SourceStatsStatus::Unsupported {
            if let Some(previous) =
                previous.filter(|value| value.status == SourceStatsStatus::Available)
            {
                let mut retained = previous.clone();
                retained.stale = true;
                retained.refresh_error = Some(self.status);
                return retained;
            }
        }
        self
    }

    pub fn empty(provider: SourceStatsProvider, status: SourceStatsStatus) -> Self {
        Self {
            provider,
            status,
            balance_micro_usd: None,
            spent_micro_usd: None,
            requests: None,
            total_tokens: None,
            balance_kind: SourceBalanceKind::Wallet,
            balance_unlimited: false,
            amounts: Vec::new(),
            as_of_ms: None,
            stale: false,
            refresh_error: None,
        }
    }

    fn amount(&mut self, currency: SourceStatsCurrency, balance: Option<i64>, spent: Option<i64>) {
        if currency == SourceStatsCurrency::Usd {
            self.balance_micro_usd = balance;
            self.spent_micro_usd = spent;
        }
        if balance.is_some() || spent.is_some() {
            self.amounts.push(SourceStatsAmount {
                currency,
                balance_micros: balance,
                spent_micros: spent,
            });
        }
    }
}

pub async fn fetch_source_provider_stats(
    base_url: &str,
    api_key: &str,
) -> Result<SourceProviderStats, String> {
    read_source_provider_stats(base_url, api_key).await.value
}

pub async fn read_source_provider_stats(
    base_url: &str,
    api_key: &str,
) -> super::SourceRead<Result<SourceProviderStats, String>> {
    read_source_provider_stats_with_scope(base_url, api_key, ManagementHttpScope::default()).await
}

pub async fn read_source_provider_stats_with_scope(
    base_url: &str,
    api_key: &str,
    scope: ManagementHttpScope,
) -> super::SourceRead<Result<SourceProviderStats, String>> {
    let client = match StatsClient::new_with_scope(base_url, api_key, scope) {
        Ok(client) => client,
        Err(error) => {
            return super::SourceRead {
                value: Err(error),
                retry_after_ms: None,
            }
        }
    };
    let provider = source_stats_provider(base_url);
    let result =
        tokio::time::timeout(Duration::from_secs(20), fetch_stats(&client, provider)).await;
    let value = Ok(match result {
        Ok(Ok(stats)) => stats,
        Ok(Err(status)) => SourceProviderStats::empty(provider, status),
        Err(_) => SourceProviderStats::empty(provider, SourceStatsStatus::Unavailable),
    });
    super::SourceRead {
        value,
        retry_after_ms: client.hints.delay(),
    }
}

async fn fetch_stats(
    client: &StatsClient,
    provider: SourceStatsProvider,
) -> StatsResult<SourceProviderStats> {
    use SourceStatsProvider as P;
    match provider {
        P::Zenith => formats::zenith_stats(&client.get("zenith/key/stats", false, true).await?),
        P::OpenRouter => {
            let payload = client.get("key", false, true).await?;
            let key = formats::openrouter_key_stats(&payload)?;
            if payload
                .pointer("/data/is_management_key")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                // Ordinary inference keys cannot query account-wide credits.
                if let Ok(wallet) = client.get("credits", false, true).await {
                    if let Ok(stats) = formats::openrouter_stats(&wallet) {
                        return Ok(stats);
                    }
                }
            }
            Ok(key)
        }
        P::Deepseek => formats::deepseek_stats(&client.get("user/balance", true, true).await?),
        // SiliconFlow retired /user/info on 2026-08-14 and has not announced
        // a replacement account endpoint. Do not send a key to a dead API.
        P::SiliconFlow => Err(SourceStatsStatus::Unsupported),
        _ => autodetect(client).await,
    }
}

async fn autodetect(client: &StatsClient) -> StatsResult<SourceProviderStats> {
    use SourceStatsStatus as S;
    if matches!(
        client.host(),
        Some("api.openai.com" | "api.anthropic.com" | "generativelanguage.googleapis.com")
    ) {
        return Err(S::Unsupported);
    }
    let mut failure = S::Unsupported;
    match client.get("usage", false, true).await {
        Ok(payload) => match formats::sub2api_stats(&payload) {
            Ok(stats) => return Ok(stats),
            Err(S::Unsupported) => {}
            Err(status) => {
                return Ok(SourceProviderStats::empty(
                    SourceStatsProvider::Sub2Api,
                    status,
                ))
            }
        },
        Err(S::RateLimited) => return Err(S::RateLimited),
        Err(status) => remember_failure(&mut failure, status),
    }
    match client.get("api/usage/token/", true, true).await {
        Ok(payload) if formats::is_new_api(&payload) => {
            let metadata = client.get("api/status", true, false).await.ok();
            return Ok(
                formats::new_api_stats(&payload, metadata.as_ref()).unwrap_or_else(|status| {
                    SourceProviderStats::empty(SourceStatsProvider::NewApi, status)
                }),
            );
        }
        Ok(_) => {}
        Err(S::RateLimited) => return Err(S::RateLimited),
        Err(status) => remember_failure(&mut failure, status),
    }
    for site_path in [true, false] {
        match client
            .get("dashboard/billing/subscription", site_path, true)
            .await
        {
            Ok(payload) if formats::is_billing(&payload) => {
                let usage = client
                    .get("dashboard/billing/usage", site_path, true)
                    .await?;
                let metadata = client.get("api/status", true, false).await.ok();
                return Ok(formats::billing_stats(&payload, &usage, metadata.as_ref())
                    .unwrap_or_else(|status| {
                        SourceProviderStats::empty(SourceStatsProvider::Billing, status)
                    }));
            }
            Ok(_) => {}
            Err(S::RateLimited) => return Err(S::RateLimited),
            Err(status) => remember_failure(&mut failure, status),
        }
    }
    Err(failure)
}

fn remember_failure(current: &mut SourceStatsStatus, next: SourceStatsStatus) {
    use SourceStatsStatus as S;
    let rank = |status| match status {
        S::Unauthorized => 3,
        S::Unavailable => 2,
        S::InvalidResponse => 1,
        _ => 0,
    };
    if rank(next) > rank(*current) {
        *current = next;
    }
}

pub(super) fn source_stats_provider(base_url: &str) -> SourceStatsProvider {
    let Ok(url) = Url::parse(base_url) else {
        return SourceStatsProvider::Unsupported;
    };
    match url.host_str().map(str::to_ascii_lowercase).as_deref() {
        Some("api.zenithmarket.dev") => SourceStatsProvider::Zenith,
        Some("openrouter.ai") => SourceStatsProvider::OpenRouter,
        Some("api.deepseek.com") => SourceStatsProvider::Deepseek,
        Some("api.siliconflow.cn" | "api.siliconflow.com") => SourceStatsProvider::SiliconFlow,
        _ => SourceStatsProvider::Unsupported,
    }
}

#[cfg(test)]
pub(super) fn source_stats_endpoint(
    provider: SourceStatsProvider,
    base_url: &str,
) -> Result<Url, String> {
    let client = StatsClient::new(base_url, "synthetic")?;
    let path = match provider {
        SourceStatsProvider::Zenith => "zenith/key/stats",
        SourceStatsProvider::OpenRouter => "key",
        _ => return Err("source does not provide stats".to_owned()),
    };
    client
        .endpoint(path, false)
        .map_err(|_| "source stats URL is invalid".to_owned())
}

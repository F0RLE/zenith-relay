use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;

const MAX_SOURCE_STATS_BYTES: usize = 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatsProvider {
    Zenith,
    #[serde(rename = "openrouter")]
    OpenRouter,
    Unsupported,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceProviderStats {
    pub provider: SourceStatsProvider,
    pub balance_micro_usd: Option<i64>,
    pub spent_micro_usd: Option<i64>,
    pub requests: Option<u64>,
    pub total_tokens: Option<u64>,
}

pub async fn fetch_source_provider_stats(
    base_url: &str,
    api_key: &str,
) -> std::result::Result<SourceProviderStats, String> {
    let provider = source_stats_provider(base_url);
    let endpoint = match provider {
        SourceStatsProvider::Zenith | SourceStatsProvider::OpenRouter => {
            source_stats_endpoint(provider, base_url)?
        }
        SourceStatsProvider::Unsupported => {
            return Ok(SourceProviderStats {
                provider,
                balance_micro_usd: None,
                spent_micro_usd: None,
                requests: None,
                total_tokens: None,
            });
        }
    };
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| "source stats request could not be initialized".to_string())?;
    let response = client
        .get(endpoint)
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|_| "source stats request failed".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "source stats request failed ({})",
            response.status().as_u16()
        ));
    }
    let payload: serde_json::Value = serde_json::from_slice(&bounded_response(response).await?)
        .map_err(|_| "source stats response is invalid".to_string())?;
    match provider {
        SourceStatsProvider::Zenith => zenith_stats(&payload),
        SourceStatsProvider::OpenRouter => openrouter_stats(&payload),
        SourceStatsProvider::Unsupported => unreachable!(),
    }
}

pub(super) fn source_stats_provider(base_url: &str) -> SourceStatsProvider {
    let Ok(url) = Url::parse(base_url) else {
        return SourceStatsProvider::Unsupported;
    };
    match url.host_str().map(str::to_ascii_lowercase).as_deref() {
        Some("api.zenithmarket.dev") => SourceStatsProvider::Zenith,
        Some("openrouter.ai") => SourceStatsProvider::OpenRouter,
        _ => SourceStatsProvider::Unsupported,
    }
}

async fn bounded_response(response: reqwest::Response) -> std::result::Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SOURCE_STATS_BYTES as u64)
    {
        return Err("source stats response is too large".to_string());
    }
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "source stats response could not be read".to_string())?;
        if bytes.len().saturating_add(chunk.len()) > MAX_SOURCE_STATS_BYTES {
            return Err("source stats response is too large".to_string());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(super) fn zenith_stats(
    payload: &serde_json::Value,
) -> std::result::Result<SourceProviderStats, String> {
    let data = payload.get("data").unwrap_or(payload);
    let balance_micro_usd = money_field(
        data,
        &["displayBalanceMicrousd", "balanceMicrousd"],
        "balanceCents",
    );
    let spent_micro_usd = money_field(
        data,
        &["displaySpentMicrousd", "spentMicrousd"],
        "spentCents",
    );
    if balance_micro_usd.is_none() || spent_micro_usd.is_none() {
        return Err("Zenith source stats are incomplete".to_string());
    }
    Ok(SourceProviderStats {
        provider: SourceStatsProvider::Zenith,
        balance_micro_usd,
        spent_micro_usd,
        requests: unsigned_field(data, &["requests"]),
        total_tokens: unsigned_field(data, &["totalTokens", "total_tokens"]),
    })
}

pub(super) fn source_stats_endpoint(
    provider: SourceStatsProvider,
    base_url: &str,
) -> std::result::Result<Url, String> {
    let mut endpoint =
        Url::parse(base_url).map_err(|_| "source stats base URL is invalid".to_string())?;
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    if !endpoint.path().ends_with('/') {
        endpoint.set_path(&format!("{}/", endpoint.path()));
    }
    let path = match provider {
        SourceStatsProvider::Zenith => "zenith/key/stats",
        SourceStatsProvider::OpenRouter => "credits",
        SourceStatsProvider::Unsupported => return Err("source does not provide stats".to_string()),
    };
    endpoint
        .join(path)
        .map_err(|_| "source stats URL is invalid".to_string())
}

pub(super) fn openrouter_stats(
    payload: &serde_json::Value,
) -> std::result::Result<SourceProviderStats, String> {
    let data = payload.get("data").unwrap_or(payload);
    let total_credits = number_field(data, &["total_credits", "totalCredits"])
        .ok_or_else(|| "OpenRouter credits are missing".to_string())?;
    let total_usage = number_field(data, &["total_usage", "totalUsage"])
        .ok_or_else(|| "OpenRouter usage is missing".to_string())?;
    Ok(SourceProviderStats {
        provider: SourceStatsProvider::OpenRouter,
        balance_micro_usd: usd_to_micro_usd(total_credits - total_usage),
        spent_micro_usd: usd_to_micro_usd(total_usage),
        requests: None,
        total_tokens: None,
    })
}

fn money_field(data: &serde_json::Value, micro_names: &[&str], cents_name: &str) -> Option<i64> {
    signed_field(data, micro_names)
        .or_else(|| signed_field(data, &[cents_name]).map(|cents| cents.saturating_mul(10_000)))
}

fn signed_field(value: &serde_json::Value, names: &[&str]) -> Option<i64> {
    names.iter().find_map(|name| {
        value.get(name).and_then(|value| {
            value
                .as_i64()
                .or_else(|| i64::try_from(value.as_u64()?).ok())
        })
    })
}

fn unsigned_field(value: &serde_json::Value, names: &[&str]) -> Option<u64> {
    names.iter().find_map(|name| {
        value.get(name).and_then(|value| {
            value
                .as_u64()
                .or_else(|| u64::try_from(value.as_i64()?).ok())
        })
    })
}

fn number_field(value: &serde_json::Value, names: &[&str]) -> Option<f64> {
    names.iter().find_map(|name| value.get(name)?.as_f64())
}

fn usd_to_micro_usd(value: f64) -> Option<i64> {
    let value = (value * 1_000_000.0).round();
    (value.is_finite() && value >= i64::MIN as f64 && value <= i64::MAX as f64)
        .then_some(value as i64)
}

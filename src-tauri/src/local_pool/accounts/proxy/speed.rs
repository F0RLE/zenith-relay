use serde::Serialize;
use std::time::{Duration, Instant};
use zenith_relay_core::scheduler::refresh::http::{HttpClass, management_http_gate};
use zenith_relay_core::{ProxyConfig, error_codes};

const DOWNLOAD_URL: &str = "https://speed.cloudflare.com/__down?bytes=5242880";
const DOWNLOAD_BYTES: u64 = 5 * 1024 * 1024;
const BUDGET: Duration = Duration::from_secs(12);
// Queue measurements rather than competing for bandwidth or rejecting busy tests.
static SPEED_LIMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxySpeedTestResult {
    pub proxy_id: String,
    pub downloaded_bytes: u64,
    pub elapsed_ms: u64,
    pub download_mbps: Option<f64>,
    pub error_code: Option<String>,
}

pub async fn test(
    proxy_id: String,
    proxy: &ProxyConfig,
    on_started: impl FnOnce() -> bool,
) -> ProxySpeedTestResult {
    test_at(proxy_id, proxy, DOWNLOAD_URL, BUDGET, on_started).await
}

async fn test_at(
    proxy_id: String,
    proxy: &ProxyConfig,
    url: &str,
    budget: Duration,
    on_started: impl FnOnce() -> bool,
) -> ProxySpeedTestResult {
    let mut result = ProxySpeedTestResult {
        proxy_id,
        ..ProxySpeedTestResult::default()
    };
    let Ok(_slot) = SPEED_LIMIT.acquire().await else {
        result.error_code = Some(error_codes::PROXY_CHECK_UNAVAILABLE.into());
        return result;
    };
    if !on_started() {
        result.error_code = Some(error_codes::PROXY_CHECK_UNAVAILABLE.into());
        return result;
    }
    // Waiting in the queue must not consume the download's timeout budget.
    match tokio::time::timeout(budget, download(proxy, url, budget)).await {
        Ok(Ok((bytes, elapsed))) => {
            result.downloaded_bytes = bytes;
            result.elapsed_ms = elapsed.as_millis().try_into().unwrap_or(u64::MAX);
            result.download_mbps = Some(megabits_per_second(bytes, elapsed));
        }
        measurement => {
            result.error_code = Some(
                match measurement {
                    Ok(Err(code)) => code,
                    _ => error_codes::PROXY_CHECK_TIMEOUT,
                }
                .into(),
            );
        }
    }
    result
}

fn megabits_per_second(bytes: u64, elapsed: Duration) -> f64 {
    bytes as f64 * 8.0 / elapsed.as_secs_f64().max(0.001) / 1_000_000.0
}

async fn download(
    proxy: &ProxyConfig,
    url: &str,
    budget: Duration,
) -> Result<(u64, Duration), &'static str> {
    // Only the selected proxy receives its proxy credentials. Never use an
    // environment proxy, account/API key, redirect, or direct fallback.
    let client = proxy
        .apply(reqwest::Client::builder().no_proxy())
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(budget.min(Duration::from_secs(6)))
        .timeout(budget)
        .build()
        .map_err(|_| error_codes::PROXY_CHECK_CONNECTION_FAILED)?;
    let (mut response, _permit) = management_http_gate()
        .send(
            &client,
            client
                .get(url)
                .header(reqwest::header::ACCEPT_ENCODING, "identity")
                .header(reqwest::header::CACHE_CONTROL, "no-store"),
            HttpClass::Ordinary,
        )
        .await
        .map_err(|error| {
            if error.is_timeout() {
                error_codes::PROXY_CHECK_TIMEOUT
            } else {
                error_codes::PROXY_CHECK_CONNECTION_FAILED
            }
        })?;
    if response.status() == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED {
        return Err(error_codes::PROXY_CHECK_AUTH_FAILED);
    }
    if response.status() != reqwest::StatusCode::OK {
        return Err(error_codes::PROXY_CHECK_REJECTED);
    }
    let binary = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/octet-stream"))
        });
    let uncompressed = response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .is_none_or(|value| value.as_bytes().eq_ignore_ascii_case(b"identity"));
    if !binary
        || !uncompressed
        || response
            .content_length()
            .is_some_and(|size| size != DOWNLOAD_BYTES)
    {
        return Err(error_codes::PROXY_CHECK_INVALID_RESPONSE);
    }
    // Measure payload transfer, excluding HTTP admission and connection setup.
    // Count streamed bytes without retaining the download in memory or on disk.
    let started = Instant::now();
    let mut bytes = 0_u64;
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        if error.is_timeout() {
            error_codes::PROXY_CHECK_TIMEOUT
        } else {
            error_codes::PROXY_CHECK_CONNECTION_FAILED
        }
    })? {
        bytes = bytes.saturating_add(chunk.len() as u64);
        if bytes > DOWNLOAD_BYTES {
            return Err(error_codes::PROXY_CHECK_INVALID_RESPONSE);
        }
    }
    if bytes != DOWNLOAD_BYTES {
        return Err(error_codes::PROXY_CHECK_INVALID_RESPONSE);
    }
    Ok((bytes, started.elapsed().max(Duration::from_millis(1))))
}

#[cfg(test)]
mod tests;

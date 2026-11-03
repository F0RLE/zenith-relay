use serde::{Deserialize, Serialize};
use std::{
    net::IpAddr,
    time::{Duration, Instant},
};
use zenith_relay_core::scheduler::refresh::http::{HttpClass, management_http_gate};
use zenith_relay_core::{ProxyConfig, error_codes};

const CHECK_URL: &str = "https://www.cloudflare.com/cdn-cgi/trace";
const MAX_RESPONSE_BYTES: usize = 4_096;
static CHECK_LIMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

mod lookup;
mod secondary;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyCheckResult {
    pub proxy_id: String,
    pub checked_at_ms: u64,
    pub elapsed_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<ProxyCheckTiming>,
    pub ip: Option<IpAddr>,
    #[serde(default)]
    pub ipv4: Option<std::net::Ipv4Addr>,
    #[serde(default)]
    pub ipv6: Option<std::net::Ipv6Addr>,
    #[serde(default)]
    pub secondary_check_error: Option<String>,
    pub country_code: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub city: Option<String>,
    #[serde(default)]
    pub location_source: Option<String>,
    #[serde(default)]
    pub location_error_code: Option<String>,
    pub error_code: Option<String>,
}

/// Probe offsets start after the check queue; total includes queue and optional
/// location/address work. Headers include setup/gate/upstream wait, not just TCP.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyCheckTiming {
    pub queue_ms: u64,
    pub total_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_headers_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_complete_ms: Option<u64>,
}

pub async fn check(proxy_id: String, proxy: &ProxyConfig, checked_at_ms: u64) -> ProxyCheckResult {
    check_at(
        proxy_id,
        proxy,
        checked_at_ms,
        CHECK_URL,
        lookup::BASE_URL,
        Some(("https://api.ipify.org/", "https://api6.ipify.org/")),
    )
    .await
}

async fn check_at(
    proxy_id: String,
    proxy: &ProxyConfig,
    checked_at_ms: u64,
    check_url: &str,
    location_base: &str,
    family_endpoints: Option<(&str, &str)>,
) -> ProxyCheckResult {
    let total_started = Instant::now();
    let _permit = CHECK_LIMIT
        .acquire()
        .await
        .expect("proxy check limiter is never closed");
    let started = Instant::now();
    let mut timing = ProxyCheckTiming {
        queue_ms: total_started
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
        ..Default::default()
    };
    let budget = Duration::from_secs(12);
    // Measure only the bounded HTTPS exit-IP probe, never optional geolocation.
    let probe = tokio::time::timeout(
        budget,
        proxy_check_request(proxy, check_url, budget, started, &mut timing),
    )
    .await
    .unwrap_or(Err(error_codes::PROXY_CHECK_TIMEOUT));
    let elapsed_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    let mut result = ProxyCheckResult {
        proxy_id,
        checked_at_ms,
        elapsed_ms,
        ..ProxyCheckResult::default()
    };
    match probe {
        Ok((ip, country_code)) => {
            result.ip = Some(ip);
            secondary::set_address(&mut result, ip);
            result.country_code = country_code;
            result.location_source = result.country_code.as_ref().map(|_| "cloudflare".into());
            let primary_location = lookup::lookup(
                proxy,
                ip,
                result.country_code.as_deref(),
                location_base,
                Duration::from_secs(3),
            );
            let additional = async {
                let (v4, v6) = family_endpoints?;
                Some(
                    secondary::probe(
                        proxy,
                        ip.is_ipv6(),
                        if ip.is_ipv4() { v6 } else { v4 },
                        location_base,
                    )
                    .await,
                )
            };
            let (primary_location, additional) = tokio::join!(primary_location, additional);
            match primary_location {
                Ok(location) => {
                    result.country_code = Some(location.country_code);
                    result.region = location.region;
                    result.city = location.city;
                    result.timezone = location.timezone;
                    result.location_source = Some("ipwhois".into());
                }
                Err(code) => result.location_error_code = Some(code.into()),
            }
            match additional {
                Some(Ok((address, location))) => {
                    secondary::set_address(&mut result, address);
                    match location {
                        Ok(location) => secondary::check_location(&mut result, &location),
                        Err(error) => result.secondary_check_error = Some(error.into()),
                    }
                }
                Some(Err(error)) => result.secondary_check_error = Some(error.into()),
                None => {}
            }
        }
        Err(code) => result.error_code = Some(code.to_owned()),
    }
    timing.total_ms = total_started
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    result.timing = Some(timing);
    result
}

async fn proxy_check_request(
    proxy: &ProxyConfig,
    url: &str,
    timeout: Duration,
    started: Instant,
    timing: &mut ProxyCheckTiming,
) -> Result<(IpAddr, Option<String>), &'static str> {
    parse_trace(&proxy_check_body_observed(proxy, url, timeout, started, Some(timing)).await?)
}

async fn proxy_check_body(
    proxy: &ProxyConfig,
    url: &str,
    timeout: Duration,
) -> Result<Vec<u8>, &'static str> {
    proxy_check_body_observed(proxy, url, timeout, Instant::now(), None).await
}

async fn proxy_check_body_observed(
    proxy: &ProxyConfig,
    url: &str,
    timeout: Duration,
    started: Instant,
    mut timing: Option<&mut ProxyCheckTiming>,
) -> Result<Vec<u8>, &'static str> {
    // Do not inherit environment proxies, follow redirects, or retry directly.
    // Only the chosen proxy receives its credentials; no account/API key is used.
    let client = proxy
        .apply(reqwest::Client::builder().no_proxy())
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(timeout.min(Duration::from_secs(6)))
        .timeout(timeout)
        .build()
        .map_err(|_| error_codes::PROXY_CHECK_CONNECTION_FAILED)?;
    let (mut proxy_check_response, permit) = management_http_gate()
        .send(&client, client.get(url), HttpClass::Ordinary)
        .await
        .map_err(|error| {
            if error.is_timeout() {
                error_codes::PROXY_CHECK_TIMEOUT
            } else {
                error_codes::PROXY_CHECK_CONNECTION_FAILED
            }
        })?;
    if let Some(timing) = timing.as_mut() {
        timing.response_headers_ms =
            Some(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX));
    }
    if proxy_check_response.status() == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED {
        return Err(error_codes::PROXY_CHECK_AUTH_FAILED);
    }
    if !proxy_check_response.status().is_success() {
        return Err(error_codes::PROXY_CHECK_REJECTED);
    }
    if proxy_check_response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return Err(error_codes::PROXY_CHECK_INVALID_RESPONSE);
    }
    let mut response_bytes = Vec::new();
    while let Some(chunk) = proxy_check_response.chunk().await.map_err(classify_error)? {
        if response_bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(error_codes::PROXY_CHECK_INVALID_RESPONSE);
        }
        response_bytes.extend_from_slice(&chunk);
    }
    drop(permit);
    if let Some(timing) = timing {
        timing.response_complete_ms =
            Some(started.elapsed().as_millis().try_into().unwrap_or(u64::MAX));
    }
    Ok(response_bytes)
}

fn classify_error(error: reqwest::Error) -> &'static str {
    if error.is_timeout() {
        error_codes::PROXY_CHECK_TIMEOUT
    } else {
        error_codes::PROXY_CHECK_CONNECTION_FAILED
    }
}

fn parse_trace(trace_body: &[u8]) -> Result<(IpAddr, Option<String>), &'static str> {
    let text =
        std::str::from_utf8(trace_body).map_err(|_| error_codes::PROXY_CHECK_INVALID_RESPONSE)?;
    let mut ip = None;
    let mut country = None;
    for line in text.lines() {
        if let Some(ip_value) = line.strip_prefix("ip=") {
            if ip.is_some() {
                return Err(error_codes::PROXY_CHECK_INVALID_RESPONSE);
            }
            ip = Some(
                ip_value
                    .parse()
                    .map_err(|_| error_codes::PROXY_CHECK_INVALID_RESPONSE)?,
            );
        } else if let Some(country_value) = line.strip_prefix("loc=")
            && country_value.len() == 2
            && country_value.bytes().all(|byte| byte.is_ascii_uppercase())
            && country_value != "XX"
        {
            country = Some(country_value.to_string());
        }
    }
    ip.map(|ip| (ip, country))
        .ok_or(error_codes::PROXY_CHECK_INVALID_RESPONSE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        http::{HeaderMap, StatusCode},
        routing::get,
    };

    #[test]
    fn trace_retains_only_valid_exit_metadata() {
        assert_eq!(
            parse_trace(b"ip=203.0.113.9\nloc=NL\nother=ignored\n").unwrap(),
            ("203.0.113.9".parse().unwrap(), Some("NL".into()))
        );
        assert_eq!(parse_trace(b"ip=2001:db8::1\nloc=XX\n").unwrap().1, None);
        for trace_body in [
            b"ip=not-an-ip\n".as_slice(),
            b"loc=NL\n",
            b"ip=203.0.113.9\nip=203.0.113.10\n",
            b"<html>error</html>",
        ] {
            assert_eq!(
                parse_trace(trace_body).unwrap_err(),
                error_codes::PROXY_CHECK_INVALID_RESPONSE
            );
        }
    }

    #[tokio::test]
    async fn country_is_available_without_database_or_optional_geolocation() {
        let (proxy, server) = mock_proxy().await;
        let result = check_at(
            "synthetic".into(),
            &proxy,
            1,
            "http://unreachable.invalid/trace",
            "http://unreachable.invalid/unavailable/",
            None,
        )
        .await;
        assert_eq!(result.ip.unwrap().to_string(), "203.0.113.9");
        assert!(result.error_code.is_none());
        assert_eq!(result.country_code.as_deref(), Some("NL"));
        assert_eq!(result.location_source.as_deref(), Some("cloudflare"));
        assert!(result.timezone.is_none());
        assert_eq!(
            result.location_error_code.as_deref(),
            Some("proxy_location_unavailable")
        );
        server.abort();
    }

    #[test]
    fn legacy_saved_checks_remain_readable_without_claiming_observed_geography() {
        let result: ProxyCheckResult = serde_json::from_value(serde_json::json!({
            "proxyId": "synthetic", "checkedAtMs": 1, "elapsedMs": 5,
            "ip": "203.0.113.9", "countryCode": "NL", "timezone": "Europe/Amsterdam", "errorCode": null,
        })).unwrap();
        assert!(result.location_source.is_none());
        assert!(result.timing.is_none());
        assert_eq!(result.timezone.as_deref(), Some("Europe/Amsterdam"));
    }

    #[tokio::test]
    async fn automatic_city_lookup_does_not_inflate_the_https_probe_duration() {
        let (proxy, server) = mock_proxy().await;
        let started = Instant::now();
        let result = check_at(
            "synthetic".into(),
            &proxy,
            1,
            "http://unreachable.invalid/trace",
            "http://unreachable.invalid/",
            None,
        )
        .await;
        assert!(result.error_code.is_none());
        assert_eq!(result.country_code.as_deref(), Some("NL"));
        assert_eq!(result.city.as_deref(), Some("Amsterdam"));
        assert_eq!(result.timezone.as_deref(), Some("Europe/Amsterdam"));
        assert_eq!(result.location_source.as_deref(), Some("ipwhois"));
        assert!(started.elapsed().as_millis() >= u128::from(result.elapsed_ms) + 40);
        let timing = result.timing.unwrap();
        assert!(timing.response_headers_ms.is_some());
        assert!(timing.response_complete_ms.unwrap() >= timing.response_headers_ms.unwrap());
        assert!(timing.response_complete_ms.unwrap() <= result.elapsed_ms);
        assert!(timing.total_ms >= result.elapsed_ms + timing.queue_ms + 40);
        server.abort();
    }

    pub(super) async fn mock_proxy() -> (ProxyConfig, tokio::task::JoinHandle<()>) {
        let app = Router::new()
            .route(
                "/ipv6",
                get(|headers: HeaderMap| async move {
                    assert!(headers.contains_key("proxy-authorization"));
                    assert!(!headers.contains_key("authorization"));
                    "2001:db8::1"
                }),
            )
            .route(
                "/2001:db8::1",
                get(|| async {
                    axum::Json(serde_json::json!({"success": true, "ip": "2001:db8::1",
                    "country_code": "US", "region": "New York", "city": "New York",
                    "timezone": {"id": "America/New_York"}}))
                }),
            )
            .route(
                "/203.0.113.9",
                get(|| async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    axum::Json(serde_json::json!({
                        "success": true, "ip": "203.0.113.9", "country_code": "NL",
                        "city": "Amsterdam", "timezone": {"id": "Europe/Amsterdam"},
                    }))
                }),
            )
            .route(
                "/trace",
                get(|headers: HeaderMap| async move {
                    assert!(headers.contains_key("proxy-authorization"));
                    assert!(!headers.contains_key("authorization"));
                    "ip=203.0.113.9\nloc=NL\n"
                }),
            )
            .route(
                "/auth",
                get(|| async { StatusCode::PROXY_AUTHENTICATION_REQUIRED }),
            )
            .route(
                "/redirect",
                get(|| async {
                    (
                        StatusCode::FOUND,
                        [("location", "http://unreachable.invalid/trace")],
                    )
                }),
            )
            .route("/invalid", get(|| async { "upstream private message" }))
            .route(
                "/large",
                get(|| async { "x".repeat(MAX_RESPONSE_BYTES + 1) }),
            )
            .route(
                "/slow",
                get(|| async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    "ip=203.0.113.9\n"
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = ProxyConfig::parse(&format!(
            "http://synthetic:secret@{}",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (proxy, task)
    }

    #[tokio::test]
    async fn check_uses_selected_proxy_and_bounds_failures() {
        let (proxy, server) = mock_proxy().await;
        let mut timing = ProxyCheckTiming::default();
        let check_result = proxy_check_request(
            &proxy,
            "http://unreachable.invalid/trace",
            Duration::from_secs(2),
            Instant::now(),
            &mut timing,
        )
        .await
        .unwrap();
        assert_eq!(check_result.0.to_string(), "203.0.113.9");
        assert!(timing.response_headers_ms.is_some());
        assert!(timing.response_complete_ms.unwrap() >= timing.response_headers_ms.unwrap());
        for (path, expected) in [
            ("auth", error_codes::PROXY_CHECK_AUTH_FAILED),
            ("redirect", error_codes::PROXY_CHECK_REJECTED),
            ("invalid", error_codes::PROXY_CHECK_INVALID_RESPONSE),
            ("large", error_codes::PROXY_CHECK_INVALID_RESPONSE),
            ("slow", error_codes::PROXY_CHECK_TIMEOUT),
        ] {
            assert_eq!(
                proxy_check_request(
                    &proxy,
                    &format!("http://unreachable.invalid/{path}"),
                    Duration::from_millis(100),
                    Instant::now(),
                    &mut ProxyCheckTiming::default(),
                )
                .await
                .unwrap_err(),
                expected
            );
        }
        server.abort();
    }

    #[tokio::test]
    async fn unavailable_proxy_does_not_fall_back_to_direct_connection() {
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let received = hits.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().route(
                    "/trace",
                    get(move || {
                        let received = received.clone();
                        async move {
                            received.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            "ip=203.0.113.9\n"
                        }
                    }),
                ),
            )
            .await
            .unwrap();
        });
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = ProxyConfig::parse(&format!(
            "http://synthetic:secret@{}",
            closed.local_addr().unwrap()
        ))
        .unwrap();
        drop(closed);
        let mut timing = ProxyCheckTiming::default();
        let code = proxy_check_request(
            &proxy,
            &format!("http://{address}/trace"),
            Duration::from_millis(250),
            Instant::now(),
            &mut timing,
        )
        .await
        .unwrap_err();
        assert!(
            code == error_codes::PROXY_CHECK_CONNECTION_FAILED
                || code == error_codes::PROXY_CHECK_TIMEOUT
        );
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(timing.response_headers_ms.is_none());
        assert!(timing.response_complete_ms.is_none());
        server.abort();
    }
}

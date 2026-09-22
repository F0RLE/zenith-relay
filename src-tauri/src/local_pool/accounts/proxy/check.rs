use serde::Serialize;
use std::{
    net::IpAddr,
    time::{Duration, Instant},
};
use zenith_relay_core::{error_codes, ProxyConfig};

const CHECK_URL: &str = "https://www.cloudflare.com/cdn-cgi/trace";
const MAX_RESPONSE_BYTES: usize = 4_096;
static CHECK_LIMIT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(4);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyCheckResult {
    pub proxy_id: String,
    pub checked_at_ms: u64,
    pub elapsed_ms: u64,
    pub ip: Option<IpAddr>,
    pub country_code: Option<String>,
    pub error_code: Option<&'static str>,
}

pub async fn check(proxy_id: String, proxy: &ProxyConfig, checked_at_ms: u64) -> ProxyCheckResult {
    let _permit = CHECK_LIMIT
        .acquire()
        .await
        .expect("proxy check limiter is never closed");
    let started = Instant::now();
    let result = request(proxy, CHECK_URL, Duration::from_secs(12)).await;
    let (ip, country_code, error_code) = match result {
        Ok((ip, country)) => (Some(ip), country, None),
        Err(code) => (None, None, Some(code)),
    };
    ProxyCheckResult {
        proxy_id,
        checked_at_ms,
        elapsed_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        ip,
        country_code,
        error_code,
    }
}

async fn request(
    proxy: &ProxyConfig,
    url: &str,
    timeout: Duration,
) -> Result<(IpAddr, Option<String>), &'static str> {
    // Do not inherit environment proxies, follow redirects, or retry directly.
    // Only the chosen proxy receives its credentials; no account/API key is used.
    let client = proxy
        .apply(reqwest::Client::builder().no_proxy())
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(timeout.min(Duration::from_secs(6)))
        .timeout(timeout)
        .build()
        .map_err(|_| error_codes::PROXY_CHECK_CONNECTION_FAILED)?;
    let mut response = client.get(url).send().await.map_err(classify_error)?;
    if response.status() == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED {
        return Err(error_codes::PROXY_CHECK_AUTH_FAILED);
    }
    if !response.status().is_success() {
        return Err(error_codes::PROXY_CHECK_REJECTED);
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_RESPONSE_BYTES as u64)
    {
        return Err(error_codes::PROXY_CHECK_INVALID_RESPONSE);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(classify_error)? {
        if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(error_codes::PROXY_CHECK_INVALID_RESPONSE);
        }
        bytes.extend_from_slice(&chunk);
    }
    parse_trace(&bytes)
}

fn classify_error(error: reqwest::Error) -> &'static str {
    if error.is_timeout() {
        error_codes::PROXY_CHECK_TIMEOUT
    } else {
        error_codes::PROXY_CHECK_CONNECTION_FAILED
    }
}

fn parse_trace(bytes: &[u8]) -> Result<(IpAddr, Option<String>), &'static str> {
    let text = std::str::from_utf8(bytes).map_err(|_| error_codes::PROXY_CHECK_INVALID_RESPONSE)?;
    let mut ip = None;
    let mut country = None;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("ip=") {
            if ip.is_some() {
                return Err(error_codes::PROXY_CHECK_INVALID_RESPONSE);
            }
            ip = Some(
                value
                    .parse()
                    .map_err(|_| error_codes::PROXY_CHECK_INVALID_RESPONSE)?,
            );
        } else if let Some(value) = line.strip_prefix("loc=") {
            if value.len() == 2
                && value.bytes().all(|byte| byte.is_ascii_uppercase())
                && value != "XX"
            {
                country = Some(value.to_string());
            }
        }
    }
    ip.map(|ip| (ip, country))
        .ok_or(error_codes::PROXY_CHECK_INVALID_RESPONSE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        http::{HeaderMap, StatusCode},
        routing::get,
        Router,
    };

    #[test]
    fn trace_retains_only_valid_exit_metadata() {
        assert_eq!(
            parse_trace(b"ip=203.0.113.9\nloc=NL\nother=ignored\n").unwrap(),
            ("203.0.113.9".parse().unwrap(), Some("NL".into()))
        );
        assert_eq!(parse_trace(b"ip=2001:db8::1\nloc=XX\n").unwrap().1, None);
        for value in [
            b"ip=not-an-ip\n".as_slice(),
            b"loc=NL\n",
            b"ip=203.0.113.9\nip=203.0.113.10\n",
            b"<html>error</html>",
        ] {
            assert_eq!(
                parse_trace(value).unwrap_err(),
                error_codes::PROXY_CHECK_INVALID_RESPONSE
            );
        }
    }

    async fn mock_proxy() -> (ProxyConfig, tokio::task::JoinHandle<()>) {
        let app = Router::new()
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
        let result = request(
            &proxy,
            "http://unreachable.invalid/trace",
            Duration::from_secs(2),
        )
        .await
        .unwrap();
        assert_eq!(result.0.to_string(), "203.0.113.9");
        for (path, expected) in [
            ("auth", error_codes::PROXY_CHECK_AUTH_FAILED),
            ("redirect", error_codes::PROXY_CHECK_REJECTED),
            ("invalid", error_codes::PROXY_CHECK_INVALID_RESPONSE),
            ("large", error_codes::PROXY_CHECK_INVALID_RESPONSE),
            ("slow", error_codes::PROXY_CHECK_TIMEOUT),
        ] {
            assert_eq!(
                request(
                    &proxy,
                    &format!("http://unreachable.invalid/{path}"),
                    Duration::from_millis(100)
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
        let code = request(
            &proxy,
            &format!("http://{address}/trace"),
            Duration::from_millis(250),
        )
        .await
        .unwrap_err();
        assert!(
            code == error_codes::PROXY_CHECK_CONNECTION_FAILED
                || code == error_codes::PROXY_CHECK_TIMEOUT
        );
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
        server.abort();
    }
}

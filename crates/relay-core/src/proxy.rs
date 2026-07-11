use reqwest::ClientBuilder;
use std::fmt;
use url::Url;

const MAX_PROXY_URL_BYTES: usize = 2_048;

#[derive(Clone)]
pub struct ProxyConfig {
    proxy: reqwest::Proxy,
}

impl ProxyConfig {
    pub fn parse(value: &str) -> Result<Self, &'static str> {
        let normalized = normalize_proxy_url(value)?;
        let proxy = reqwest::Proxy::all(&normalized).map_err(|_| "proxy URL is invalid")?;
        Ok(Self { proxy })
    }

    pub fn apply(&self, builder: ClientBuilder) -> ClientBuilder {
        builder.proxy(self.proxy.clone())
    }
}

impl fmt::Debug for ProxyConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProxyConfig([redacted])")
    }
}

pub fn normalize_proxy_url(value: &str) -> Result<String, &'static str> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_PROXY_URL_BYTES || value.chars().any(char::is_control)
    {
        return Err("proxy URL is invalid");
    }
    let candidate = if value.contains("://") {
        value.to_string()
    } else {
        format!("http://{value}")
    };
    let url = Url::parse(&candidate).map_err(|_| "proxy URL is invalid")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.port().is_none()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("proxy URL must use HTTP or HTTPS with an explicit host and port");
    }
    reqwest::Proxy::all(url.as_str()).map_err(|_| "proxy URL is invalid")?;
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::StatusCode, routing::any, Router};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[test]
    fn proxy_url_accepts_popular_http_shape_and_redacts_debug() {
        let normalized = normalize_proxy_url("user:pass@proxy.example:8080").unwrap();
        assert_eq!(normalized, "http://user:pass@proxy.example:8080/");
        let proxy = ProxyConfig::parse(&normalized).unwrap();
        let rendered = format!("{proxy:?}");
        assert!(!rendered.contains("user"));
        assert!(!rendered.contains("pass"));
        assert!(normalize_proxy_url("socks5://proxy.example:1080").is_err());
        assert!(normalize_proxy_url("http://proxy.example/path").is_err());
    }

    #[tokio::test]
    async fn configured_proxy_is_used_without_direct_fallback() {
        let reached = Arc::new(AtomicBool::new(false));
        let marker = reached.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(
                listener,
                Router::new().fallback(any(move || {
                    let marker = marker.clone();
                    async move {
                        marker.store(true, Ordering::SeqCst);
                        StatusCode::NO_CONTENT
                    }
                })),
            )
            .await
            .unwrap();
        });
        let proxy = ProxyConfig::parse(&format!("http://{address}")).unwrap();
        let client = proxy.apply(reqwest::Client::builder()).build().unwrap();
        let response = client
            .get("http://direct-target.invalid/proxy-check")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert!(reached.load(Ordering::SeqCst));
        server.abort();
    }
}

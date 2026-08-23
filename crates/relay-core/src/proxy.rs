use reqwest::ClientBuilder;
use sha2::{Digest, Sha256};
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
    let (scheme, authority) = value.split_once("://").unwrap_or(("http", value));
    let candidate = format!("{scheme}://{}", normalize_proxy_authority(authority));
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

pub fn proxy_reference_id(value: &str) -> Result<String, &'static str> {
    let value = normalize_proxy_url(value)?;
    Ok(format!(
        "proxy_{}",
        hex::encode(Sha256::digest(value.as_bytes()))
    ))
}

fn normalize_proxy_authority(value: &str) -> String {
    if value
        .rsplit_once('@')
        .is_some_and(|(_, endpoint)| is_proxy_endpoint(endpoint))
    {
        return value.to_string();
    }
    if let Some((endpoint, credentials)) = value.split_once('@') {
        if is_proxy_endpoint(endpoint) && has_proxy_credentials(credentials) {
            return format!("{credentials}@{endpoint}");
        }
    }

    let mut leading = value.splitn(3, ':');
    if let (Some(host), Some(port), Some(credentials)) =
        (leading.next(), leading.next(), leading.next())
    {
        let endpoint = format!("{host}:{port}");
        if is_proxy_endpoint(&endpoint) && has_proxy_credentials(credentials) {
            return format!("{credentials}@{endpoint}");
        }
    }
    let mut trailing = value.rsplitn(3, ':');
    if let (Some(port), Some(host), Some(credentials)) =
        (trailing.next(), trailing.next(), trailing.next())
    {
        let endpoint = format!("{host}:{port}");
        if is_proxy_endpoint(&endpoint) && has_proxy_credentials(credentials) {
            return format!("{credentials}@{endpoint}");
        }
    }
    value.to_string()
}

fn is_proxy_endpoint(value: &str) -> bool {
    Url::parse(&format!("http://{value}")).is_ok_and(|url| {
        url.username().is_empty()
            && url.password().is_none()
            && url.host_str().is_some()
            && url.port().is_some()
            && matches!(url.path(), "" | "/")
            && url.query().is_none()
            && url.fragment().is_none()
    })
}

fn has_proxy_credentials(value: &str) -> bool {
    value
        .split_once(':')
        .is_some_and(|(username, password)| !username.is_empty() && !password.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        http::{header::PROXY_AUTHORIZATION, HeaderMap, StatusCode},
        routing::any,
        Router,
    };
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };

    #[test]
    fn proxy_url_accepts_popular_http_shape_and_redacts_debug() {
        let expected = "http://user:pass@proxy.example:8080/";
        for value in [
            "user:pass@proxy.example:8080",
            "proxy.example:8080:user:pass",
            "proxy.example:8080@user:pass",
            "user:pass:proxy.example:8080",
            "http://user:pass@proxy.example:8080",
        ] {
            assert_eq!(normalize_proxy_url(value).unwrap(), expected);
        }
        let provider_style = "proxy.example:8080:user__cr.us;anon.1;sessttl.5:pass";
        assert_eq!(
            normalize_proxy_url(provider_style).unwrap(),
            "http://user__cr.us%3Banon.1%3Bsessttl.5:pass@proxy.example:8080/"
        );
        assert_eq!(
            normalize_proxy_url("proxy.example:8080@user__cr.us;anon.1;sessttl.5:pass").unwrap(),
            "http://user__cr.us%3Banon.1%3Bsessttl.5:pass@proxy.example:8080/"
        );
        let proxy = ProxyConfig::parse(expected).unwrap();
        let rendered = format!("{proxy:?}");
        assert!(!rendered.contains("user"));
        assert!(!rendered.contains("pass"));
        assert!(normalize_proxy_url("socks5://proxy.example:1080").is_err());
        assert!(normalize_proxy_url("http://proxy.example/path").is_err());
    }

    #[test]
    fn proxy_reference_is_stable_across_supported_input_shapes() {
        let expected = proxy_reference_id("http://user:pass@proxy.example:8080").unwrap();
        assert_eq!(
            proxy_reference_id("proxy.example:8080:user:pass").unwrap(),
            expected
        );
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
                Router::new().fallback(any(move |headers: HeaderMap| {
                    let marker = marker.clone();
                    async move {
                        if headers
                            .get(PROXY_AUTHORIZATION)
                            .is_some_and(|value| value == "Basic dXNlcjpwYXNz")
                        {
                            marker.store(true, Ordering::SeqCst);
                            StatusCode::NO_CONTENT
                        } else {
                            StatusCode::PROXY_AUTHENTICATION_REQUIRED
                        }
                    }
                })),
            )
            .await
            .unwrap();
        });
        let proxy = ProxyConfig::parse(&format!("http://user:pass@{address}")).unwrap();
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

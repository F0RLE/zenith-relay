use super::WireApi;
use crate::{Error, Result};
use reqwest::header::HeaderValue;
use std::fmt;
use url::{Host, Url};
#[derive(Clone)]
pub struct ProviderSource {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub wire_api: WireApi,
    pub models: Vec<String>,
}

impl ProviderSource {
    pub fn validate(&self) -> Result<()> {
        require_value("source id", &self.id)?;
        require_value("source name", &self.name)?;
        require_value("source API key", &self.api_key)?;

        let url = normalized_base_url(&self.base_url)?;
        if url_has_userinfo(&url) {
            return Err(Error::Validation(
                "source base URL must not contain credentials".to_string(),
            ));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(Error::Validation(
                "source base URL must not contain a query or fragment".to_string(),
            ));
        }
        HeaderValue::from_str(&format!("Bearer {}", self.api_key)).map_err(|_| {
            Error::Validation("source API key contains invalid header characters".to_string())
        })?;
        if self.models.iter().any(|model| model.trim().is_empty()) {
            return Err(Error::Validation(
                "source model ids must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

impl fmt::Debug for ProviderSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderSource")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("base_url", &redact_url(&self.base_url))
            .field("api_key", &"[redacted]")
            .field("wire_api", &self.wire_api)
            .field("models", &self.models)
            .finish()
    }
}

#[derive(Clone)]
pub struct LocalGatewayKey {
    pub id: String,
    pub secret: String,
}

impl LocalGatewayKey {
    pub fn validate(&self) -> Result<()> {
        require_value("gateway credential id", &self.id)?;
        require_value("gateway credential secret", &self.secret)
    }
}

impl fmt::Debug for LocalGatewayKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LocalGatewayKey")
            .field("id", &self.id)
            .field("secret", &"[redacted]")
            .finish()
    }
}

pub(crate) fn normalized_base_url(base_url_text: &str) -> Result<Url> {
    let mut url = Url::parse(base_url_text.trim())
        .map_err(|_| Error::Validation("source base URL is invalid".to_string()))?;
    if !is_http_endpoint(&url) {
        return Err(Error::Validation(
            "source base URL must use HTTP or HTTPS".to_string(),
        ));
    }
    if url.scheme() == "http" && !is_loopback_url(&url) {
        return Err(Error::Validation(
            "unencrypted source base URLs are allowed only on loopback".to_string(),
        ));
    }
    // The UI asks for an API root, but provider dashboards and documentation
    // often copy a concrete endpoint such as `/v1/models` or
    // `/v1/chat/completions`. Treat those terminal paths as presentation
    // noise so discovery and request routing do not produce `/models/models`
    // or `/chat/completions/chat/completions`.
    let mut segments = url
        .path_segments()
        .map(|path_segments| {
            path_segments
                .filter(|segment| !segment.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let strip_count = if segments
        .last()
        .is_some_and(|segment| matches!(*segment, "models" | "responses" | "messages"))
    {
        1
    } else if (segments.len() >= 2
        && segments[segments.len() - 2] == "chat"
        && segments[segments.len() - 1] == "completions")
        || (segments.len() >= 2
            && segments[segments.len() - 2] == "models"
            && segments.last().is_some_and(|segment| {
                segment.ends_with(":generateContent") || segment.ends_with(":streamGenerateContent")
            }))
    {
        2
    } else {
        0
    };
    if strip_count > 0 {
        segments.truncate(segments.len() - strip_count);
        let path = if segments.is_empty() {
            "/".to_string()
        } else {
            format!("/{}/", segments.join("/"))
        };
        url.set_path(&path);
    } else if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

pub fn source_points_to_gateway(source_base_url: &str, gateway_base_url: &str) -> bool {
    let Ok(source) = normalized_base_url(source_base_url) else {
        return false;
    };
    let Ok(mut gateway) = Url::parse(gateway_base_url.trim()) else {
        return false;
    };
    if !gateway.path().ends_with('/') {
        gateway.set_path(&format!("{}/", gateway.path()));
    }
    let hosts_match =
        source
            .host_str()
            .zip(gateway.host_str())
            .is_some_and(|(source_host, gateway_host)| {
                source_host.eq_ignore_ascii_case(gateway_host)
                    || (is_loopback_url(&source) && is_loopback_url(&gateway))
            });
    hosts_match
        && source.scheme() == gateway.scheme()
        && source.port_or_known_default() == gateway.port_or_known_default()
        && source.path() == gateway.path()
}

pub fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

/// An address Relay can dial: HTTP or HTTPS with a host.
pub fn is_http_endpoint(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
}

/// A username, a password, or both in the URL authority.
pub fn url_has_userinfo(url: &Url) -> bool {
    !url.username().is_empty() || url.password().is_some()
}

/// Drops credentials, query, and fragment before a URL is written to diagnostics.
pub(crate) fn redact_url(url_text: &str) -> String {
    let Ok(mut url) = Url::parse(url_text) else {
        return "[invalid]".to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn require_value(field_name: &str, required_text: &str) -> Result<()> {
    if required_text.trim().is_empty() {
        return Err(Error::Validation(format!("{field_name} must not be empty")));
    }
    Ok(())
}

use crate::{Error, Result};
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use std::fmt;
use url::{Host, Url};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireApi {
    Responses,
    ChatCompletions,
    Messages,
}

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
        if !url.username().is_empty() || url.password().is_some() {
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
            .field("base_url", &redacted_base_url(&self.base_url))
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
        require_value("local key id", &self.id)?;
        require_value("local key secret", &self.secret)
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

pub(crate) fn normalized_base_url(value: &str) -> Result<Url> {
    let mut url = Url::parse(value.trim())
        .map_err(|_| Error::Validation("source base URL is invalid".to_string()))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(Error::Validation(
            "source base URL must use HTTP or HTTPS".to_string(),
        ));
    }
    if url.scheme() == "http" && !is_loopback(&url) {
        return Err(Error::Validation(
            "unencrypted source base URLs are allowed only on loopback".to_string(),
        ));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

fn redacted_base_url(value: &str) -> String {
    let Ok(mut url) = Url::parse(value) else {
        return "[invalid]".to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

fn require_value(name: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::Validation(format!("{name} must not be empty")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_validation_rejects_unsafe_urls_and_redacts_secrets() {
        let mut source = ProviderSource {
            id: "source-1".to_string(),
            name: "Example".to_string(),
            base_url: "ftp://example.test/v1".to_string(),
            api_key: "upstream-secret".to_string(),
            wire_api: WireApi::Responses,
            models: vec!["model-1".to_string()],
        };
        assert!(source.validate().is_err());

        source.base_url = "https://user:password@example.test/v1".to_string();
        assert!(source.validate().is_err());
        assert!(!format!("{source:?}").contains("password"));

        source.base_url = "http://example.test/v1".to_string();
        assert!(source.validate().is_err());
        source.base_url = "http://127.0.0.1:14998/v1".to_string();
        assert!(source.validate().is_ok());
        assert!(!format!("{source:?}").contains("upstream-secret"));
    }
}

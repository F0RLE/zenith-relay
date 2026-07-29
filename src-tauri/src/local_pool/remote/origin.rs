use std::fmt;
use url::{Host, Url};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PinnedOrigin {
    base: Url,
    origin: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OriginError {
    Invalid,
    CredentialsNotAllowed,
    PathNotAllowed,
    InsecureHttpBlocked,
}

impl fmt::Display for OriginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => "remote server URL is invalid",
            Self::CredentialsNotAllowed => "remote server URL must not contain credentials",
            Self::PathNotAllowed => "remote server URL must not contain a path, query, or fragment",
            Self::InsecureHttpBlocked => "remote HTTP requires the explicit insecure option",
        })
    }
}

impl std::error::Error for OriginError {}

impl PinnedOrigin {
    pub fn parse(value: &str, allow_insecure_http: bool) -> Result<Self, OriginError> {
        let mut base = Url::parse(value.trim()).map_err(|_| OriginError::Invalid)?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err(OriginError::Invalid);
        }
        if !base.username().is_empty() || base.password().is_some() {
            return Err(OriginError::CredentialsNotAllowed);
        }
        if !matches!(base.path(), "" | "/") || base.query().is_some() || base.fragment().is_some() {
            return Err(OriginError::PathNotAllowed);
        }
        if base.scheme() == "http" && !is_loopback(&base) && !allow_insecure_http {
            return Err(OriginError::InsecureHttpBlocked);
        }
        base.set_path("/");
        let origin = base.origin().ascii_serialization();
        Ok(Self { base, origin })
    }

    pub fn endpoint(&self, path: &str) -> Result<Url, OriginError> {
        if !path.starts_with('/') || path.starts_with("//") {
            return Err(OriginError::Invalid);
        }
        let url = self.base.join(path).map_err(|_| OriginError::Invalid)?;
        if url.origin().ascii_serialization() != self.origin {
            return Err(OriginError::Invalid);
        }
        Ok(url)
    }

    pub fn as_str(&self) -> &str {
        &self.origin
    }
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_blocks_unsafe_urls_and_pins_endpoint_origin() {
        assert_eq!(
            PinnedOrigin::parse("http://example.test", false).unwrap_err(),
            OriginError::InsecureHttpBlocked
        );
        assert_eq!(
            PinnedOrigin::parse("https://user:pass@example.test", false).unwrap_err(),
            OriginError::CredentialsNotAllowed
        );
        assert_eq!(
            PinnedOrigin::parse("https://example.test/prefix", false).unwrap_err(),
            OriginError::PathNotAllowed
        );
        let origin = PinnedOrigin::parse("http://127.0.0.1:14999", false).unwrap();
        assert_eq!(origin.endpoint("/state").unwrap().path(), "/state");
        assert!(origin.endpoint("//other.test/state").is_err());
    }
}

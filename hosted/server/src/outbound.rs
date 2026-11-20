//! The qualifier and text worker use the same installation-owned transport policy.
use reqwest::{
    Client, ClientBuilder,
    dns::{Addrs, Name, Resolve, Resolving},
};
use serde::Deserialize;
use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};
use url::{Host, Url};
use uuid::Uuid;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    #[serde(default)]
    pub outbound_policy: OutboundPolicy,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboundPolicy {
    #[serde(default)]
    text_sources: Vec<DirectTextSource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectTextSource {
    source: Uuid,
    endpoint: String,
    ipv4: Ipv4Addr,
}

impl OutboundPolicy {
    pub(crate) fn validate(&self) -> Result<(), &'static str> {
        let mut sources = std::collections::HashSet::new();
        if self.text_sources.len() > 128
            || self.text_sources.iter().any(|entry| {
                entry.source.is_nil()
                    || !sources.insert(entry.source)
                    || !public_ipv4(entry.ipv4)
                    || !https_endpoint(&entry.endpoint)
            })
        {
            return Err("invalid outbound text source policy");
        }
        Ok(())
    }

    pub(crate) fn text_route<'a>(
        &self,
        source: Uuid,
        endpoint: &'a str,
        proxied: bool,
    ) -> Result<TextRoute<'a>, &'static str> {
        if let Some(entry) = self
            .text_sources
            .iter()
            .find(|entry| entry.source == source)
        {
            if proxied {
                return Err("external HTTPS qualification requires a direct route");
            }
            if entry.endpoint != endpoint {
                return Err("text source differs from its direct HTTPS approval");
            }
            Ok(TextRoute {
                endpoint,
                direct_ipv4: Some(entry.ipv4),
                proxied,
            })
        } else if local_endpoint(endpoint) {
            Ok(TextRoute {
                endpoint,
                direct_ipv4: None,
                proxied,
            })
        } else {
            Err("text source has no outbound approval")
        }
    }
}

pub(crate) struct TextRoute<'a> {
    endpoint: &'a str,
    direct_ipv4: Option<Ipv4Addr>,
    proxied: bool,
}

impl<'a> TextRoute<'a> {
    pub(crate) fn endpoint(&self) -> &'a str {
        self.endpoint
    }

    pub(crate) fn direct_ipv4(&self) -> Option<Ipv4Addr> {
        self.direct_ipv4
    }

    pub(crate) fn uses_proxy(&self) -> bool {
        self.proxied
    }

    pub(crate) fn client(&self, proxy: Option<&str>) -> Result<Client, &'static str> {
        if self.proxied != proxy.is_some() {
            return Err("qualified proxy unavailable");
        }
        let builder = if let Some(ip) = self.direct_ipv4 {
            let url = Url::parse(self.endpoint).map_err(|_| "invalid HTTPS destination")?;
            let domain = url.domain().ok_or("invalid HTTPS destination")?;
            https_client(domain, SocketAddr::from((ip, 443)))
        } else {
            let builder = client_builder().dns_resolver(PinnedDns(None));
            if let Some(raw) = proxy {
                let url = Url::parse(raw).map_err(|_| "invalid proxy")?;
                if url.scheme() != "http"
                    || !url.port().is_some_and(|port| port != 0)
                    || url.path() != "/"
                    || url.query().is_some()
                    || url.fragment().is_some()
                    || !loopback_host(url.host())
                {
                    return Err("unqualified proxy");
                }
                zenith_relay_core::ProxyConfig::parse(url.as_str())
                    .map_err(|_| "invalid proxy")?
                    .apply(builder)
            } else {
                builder
            }
        };
        builder.build().map_err(|_| "text transport unavailable")
    }
}

fn client_builder() -> ClientBuilder {
    Client::builder()
        .no_proxy()
        .http1_only()
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(8))
}

fn https_client(domain: &str, address: SocketAddr) -> ClientBuilder {
    client_builder()
        .https_only(true)
        .dns_resolver(PinnedDns(Some((domain.to_owned(), address))))
}

/// An unknown hostname never falls back to the OS resolver.
struct PinnedDns(Option<(String, SocketAddr)>);

impl Resolve for PinnedDns {
    fn resolve(&self, name: Name) -> Resolving {
        let approved = self
            .0
            .as_ref()
            .filter(|(domain, _)| domain == name.as_str())
            .map(|(_, address)| *address);
        Box::pin(async move {
            let address = approved.ok_or_else(|| io::Error::other("unapproved DNS destination"))?;
            Ok(Box::new(std::iter::once(address)) as Addrs)
        })
    }
}

fn canonical_endpoint(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    (raw.len() <= 256
        && url.as_str() == raw
        && !zenith_relay_core::url_has_userinfo(&url)
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path() == "/v1")
        .then_some(url)
}

fn loopback_host(host: Option<Host<&str>>) -> bool {
    match host {
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        _ => false,
    }
}

fn local_endpoint(raw: &str) -> bool {
    canonical_endpoint(raw).is_some_and(|u| {
        u.scheme() == "http" && u.port().is_some_and(|port| port != 0) && loopback_host(u.host())
    })
}

fn https_endpoint(raw: &str) -> bool {
    canonical_endpoint(raw).is_some_and(|u| {
        u.scheme() == "https"
            && u.port().is_none()
            && u.domain().is_some_and(|domain| {
                domain.contains('.')
                    && domain.split('.').all(|label| {
                        (1..=63).contains(&label.len())
                            && label.as_bytes()[0].is_ascii_alphanumeric()
                            && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
                            && label
                                .bytes()
                                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    })
            })
    })
}

fn public_ipv4(ip: Ipv4Addr) -> bool {
    // Exclude special-use networks, including the globally reachable exceptions
    // inside them. Azure's platform virtual address is not a provider route.
    !matches!(
        ip.octets(),
        [0 | 10 | 127 | 224..=255, ..]
            | [100, 64..=127, ..]
            | [169, 254, ..]
            | [172, 16..=31, ..]
            | [192, 0, 0 | 2, _]
            | [192, 31, 196, _]
            | [192, 52, 193, _]
            | [192, 88, 99, _]
            | [192, 168, ..]
            | [192, 175, 48, _]
            | [198, 18 | 19, ..]
            | [198, 51, 100, _]
            | [203, 0, 113, _]
            | [168, 63, 129, 16]
    )
}

#[cfg(test)]
mod tests;

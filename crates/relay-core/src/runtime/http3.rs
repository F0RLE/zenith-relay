//! Opportunistic, direct-only HTTP/3. Probes never carry a routed request.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::{Client, Request, Version};
use tokio::sync::Semaphore;
use tokio::time::Instant;
use url::Url;

const MAX_ORIGINS: usize = 256;
const MAX_PROBES: usize = 4;
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const OBSERVATION_TTL: Duration = Duration::from_secs(300);

mod send;
pub(in crate::runtime) use send::execute;

#[derive(Clone, Copy, Eq, PartialEq)]
enum Capability {
    Probing,
    Ready,
    Unavailable,
}

struct Observation {
    capability: Capability,
    expires: Instant,
}

pub(super) struct Http3Transport {
    enabled: bool,
    proxy: hyper_util::client::proxy::matcher::Matcher,
    origins: Arc<Mutex<BTreeMap<(String, String), Observation>>>,
    probes: Arc<Semaphore>,
}

impl Http3Transport {
    pub(super) fn new() -> Self {
        let enabled = !std::env::var("ZENITH_RELAY_HTTP3").is_ok_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "off" | "false"
            )
        });
        let mut transport = Self::with_enabled(enabled);
        // Use reqwest's own matcher, with the same feature-unified system and
        // environment proxy rules, including NO_PROXY. QUIC cannot tunnel them.
        transport.proxy = hyper_util::client::proxy::matcher::Matcher::from_system();
        transport
    }

    fn with_enabled(enabled: bool) -> Self {
        Self {
            enabled,
            proxy: hyper_util::client::proxy::matcher::Matcher::builder().build(),
            origins: Arc::default(),
            probes: Arc::new(Semaphore::new(MAX_PROBES)),
        }
    }

    /// The current request stays on TCP until an independent HEAD probe succeeds.
    /// Never race or retry POSTs: an HTTP/3 error may follow upstream execution.
    pub(super) fn select_client(
        &self,
        request: &mut Request,
        direct: bool,
        candidate_id: &str,
        client: &Client,
    ) -> Option<Client> {
        if !self.enabled || !direct || !eligible(request) {
            return None;
        }
        let origin = request.url().origin().ascii_serialization();
        let uri = format!("{origin}/").parse::<axum::http::Uri>().ok()?;
        if self.proxy.intercept(&uri).is_some() {
            return None;
        }
        // Reuse this route's client, including its TLS configuration and isolated
        // account connection pool. A shared QUIC client would couple failures
        // between different OAuth accounts, unlike the existing TCP transport.
        let key = (candidate_id.to_owned(), origin.clone());
        let now = Instant::now();
        let mut origins = crate::poison::mutex(&self.origins);
        if let Some(observation) = origins.get(&key).filter(|entry| entry.expires > now) {
            if observation.capability == Capability::Ready {
                *request.version_mut() = Version::HTTP_3;
                return Some(client.clone());
            }
            return None;
        }
        let permit = self.probes.clone().try_acquire_owned().ok()?;
        origins.retain(|_, entry| entry.expires > now);
        if origins.len() >= MAX_ORIGINS {
            return None;
        }
        origins.insert(
            key.clone(),
            Observation {
                capability: Capability::Probing,
                expires: now + OBSERVATION_TTL,
            },
        );
        drop(origins);

        let client = client.clone();
        let observations = self.origins.clone();
        tokio::spawn(async move {
            let _permit = permit;
            // Only the configured HTTPS origin, never an Alt-Svc host, account
            // headers, cookies, path, query, prompt or request body. Any HTTP
            // status proves transport support; it does not establish API access.
            let probe = tokio::time::timeout(
                PROBE_TIMEOUT,
                client
                    .head(format!("{origin}/"))
                    .version(Version::HTTP_3)
                    .send(),
            )
            .await;
            let ready = matches!(probe, Ok(Ok(response)) if response.version() == Version::HTTP_3);
            crate::poison::mutex(&observations).insert(
                key,
                Observation {
                    capability: if ready {
                        Capability::Ready
                    } else {
                        Capability::Unavailable
                    },
                    expires: Instant::now() + OBSERVATION_TTL,
                },
            );
        });
        None
    }

    /// Affect later independent requests only; the failed generation is not replayed.
    pub(super) fn failed(&self, candidate_id: &str, url: &Url) {
        let key = (candidate_id.to_owned(), url.origin().ascii_serialization());
        if let Some(observation) = crate::poison::mutex(&self.origins).get_mut(&key) {
            observation.capability = Capability::Unavailable;
            observation.expires = Instant::now() + OBSERVATION_TTL;
        }
    }
}

fn eligible(request: &Request) -> bool {
    request.url().scheme() == "https"
        && request.url().host_str().is_some()
        && !crate::sources::url_has_userinfo(request.url())
        && request.method() == reqwest::Method::POST
        // The startup guard observes a buffered payload without changing its
        // bytes. Keep streaming bodies/trailers on their existing TCP path.
        && request.body().is_some_and(|body| body.as_bytes().is_some())
        && matches!(request.version(), Version::HTTP_11 | Version::HTTP_2)
        && !["connection", "upgrade", "transfer-encoding"]
            .iter()
            .any(|header| request.headers().contains_key(*header))
}

#[cfg(test)]
mod tests;

//! Replace damaged route-local TCP pools for future requests, never replay a send.

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::HttpBody;
use reqwest::{Client, Response, ResponseBuilderExt, Version};
use tokio::time::Instant;
use url::Url;

use crate::{ProxyConfig, Result};

const MAX_ROUTES: usize = 256;
const FAILURE_WINDOW: Duration = Duration::from_secs(120);
const HTTP1_COOLDOWN: Duration = Duration::from_secs(120);
const IDLE_TTL: Duration = Duration::from_secs(600);

type RouteKey = (String, String);
type Observations = Arc<Mutex<BTreeMap<RouteKey, Observation>>>;

struct Pool {
    client: Client,
    http1_only: bool,
}

struct Observation {
    pool: Arc<Pool>,
    touched: Instant,
    last_failure: Option<Instant>,
    failures: u8,
    http1_until: Option<Instant>,
}

#[derive(Default)]
pub(super) struct HttpRecovery {
    observations: Observations,
}

#[derive(Clone)]
pub(in crate::runtime) struct HttpPoolLease {
    key: RouteKey,
    pool: Arc<Pool>,
    observations: Observations,
    proxy: Option<ProxyConfig>,
}

impl HttpRecovery {
    pub(super) fn select(
        &self,
        candidate_id: &str,
        url: &Url,
        client: &Client,
        proxy: Option<&ProxyConfig>,
    ) -> Option<HttpPoolLease> {
        self.select_at(candidate_id, url, client, proxy, Instant::now())
    }

    fn select_at(
        &self,
        candidate_id: &str,
        url: &Url,
        client: &Client,
        proxy: Option<&ProxyConfig>,
        now: Instant,
    ) -> Option<HttpPoolLease> {
        let key = (candidate_id.to_owned(), url.origin().ascii_serialization());
        let mut observations = crate::poison::mutex(&self.observations);
        observations.retain(|_, entry| {
            Arc::strong_count(&entry.pool) > 1 || now.duration_since(entry.touched) < IDLE_TTL
        });
        if observations.len() >= MAX_ROUTES && !observations.contains_key(&key) {
            return None;
        }
        let entry = observations
            .entry(key.clone())
            .or_insert_with(|| Observation {
                pool: Arc::new(Pool {
                    client: client.clone(),
                    http1_only: false,
                }),
                touched: now,
                last_failure: None,
                failures: 0,
                http1_until: None,
            });
        if entry.http1_until.is_some_and(|until| until <= now) {
            // Do not restore the original, possibly broken connection pool.
            let client = fresh_client(proxy, false).ok()?;
            entry.pool = Arc::new(Pool {
                client,
                http1_only: false,
            });
            entry.http1_until = None;
            entry.last_failure = None;
            entry.failures = 0;
        }
        entry.touched = now;
        Some(HttpPoolLease {
            key,
            pool: entry.pool.clone(),
            observations: self.observations.clone(),
            proxy: proxy.cloned(),
        })
    }
}

impl HttpPoolLease {
    pub(in crate::runtime) fn client(&self) -> Client {
        self.pool.client.clone()
    }

    pub(in crate::runtime) fn http1_only(&self) -> bool {
        self.pool.http1_only
    }

    pub(in crate::runtime) fn observe_error(&self, error: &reqwest::Error) {
        if connection_failure(error, false) {
            self.retire(Instant::now());
        }
    }

    fn retire(&self, now: Instant) {
        let mut observations = crate::poison::mutex(&self.observations);
        let Some(entry) = observations.get_mut(&self.key) else {
            return;
        };
        // Multiplexed failures and late completions from a retired generation
        // cannot evict its replacement or count as a second independent fault.
        if !Arc::ptr_eq(&entry.pool, &self.pool) {
            return;
        }
        let failures = if entry
            .last_failure
            .is_some_and(|last| now.duration_since(last) < FAILURE_WINDOW)
        {
            entry.failures.saturating_add(1)
        } else {
            1
        };
        let http1_only = self.pool.http1_only || failures >= 2;
        let Ok(client) = fresh_client(self.proxy.as_ref(), http1_only) else {
            return;
        };
        entry.pool = Arc::new(Pool { client, http1_only });
        entry.touched = now;
        entry.failures = failures;
        entry.last_failure = Some(now);
        if http1_only && entry.http1_until.is_none() {
            entry.http1_until = Some(now + HTTP1_COOLDOWN);
        }
    }

    fn completed(&self) {
        let mut observations = crate::poison::mutex(&self.observations);
        if let Some(entry) = observations.get_mut(&self.key)
            && Arc::ptr_eq(&entry.pool, &self.pool)
            && !self.pool.http1_only
        {
            entry.failures = 0;
            entry.last_failure = None;
        }
    }

    pub(in crate::runtime) fn observe_response(self, response: Response) -> Response {
        if response.version() != Version::HTTP_2 {
            return response;
        }
        let url = response.url().clone();
        let response: axum::http::Response<reqwest::Body> = response.into();
        let (parts, body) = response.into_parts();
        let observed = ObservedBody {
            inner: body,
            lease: self,
            settled: false,
        };
        let mut builder = axum::http::Response::builder()
            .status(parts.status)
            .version(parts.version);
        *builder.headers_mut().expect("existing response headers") = parts.headers;
        *builder
            .extensions_mut()
            .expect("existing response extensions") = parts.extensions;
        builder
            .url(url)
            .body(reqwest::Body::wrap(observed))
            .expect("existing response parts remain valid")
            .into()
    }
}

struct ObservedBody {
    inner: reqwest::Body,
    lease: HttpPoolLease,
    settled: bool,
}

impl HttpBody for ObservedBody {
    type Data = <reqwest::Body as HttpBody>::Data;
    type Error = reqwest::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<hyper::body::Frame<Self::Data>, Self::Error>>> {
        let result = Pin::new(&mut self.inner).poll_frame(cx);
        if !self.settled {
            match &result {
                Poll::Ready(Some(Err(error))) => {
                    if connection_failure(error, true) {
                        self.lease.retire(Instant::now());
                    }
                    self.settled = true;
                }
                Poll::Ready(None) => {
                    self.lease.completed();
                    self.settled = true;
                }
                Poll::Ready(Some(Ok(_))) if self.inner.is_end_stream() => {
                    self.lease.completed();
                    self.settled = true;
                }
                _ => {}
            }
        }
        result
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }
    fn size_hint(&self) -> hyper::body::SizeHint {
        self.inner.size_hint()
    }
}

fn fresh_client(proxy: Option<&ProxyConfig>, http1_only: bool) -> Result<Client> {
    if http1_only {
        super::support::runtime_client_builder(proxy)
            .http1_only()
            .build()
            .map_err(Into::into)
    } else {
        super::support::runtime_client(proxy)
    }
}

fn connection_failure(error: &(dyn StdError + 'static), during_http2: bool) -> bool {
    let mut current = Some(error);
    let mut sending = false;
    while let Some(error) = current {
        if let Some(error) = error.downcast_ref::<reqwest::Error>() {
            if error.is_timeout() || error.is_builder() {
                return false;
            }
            sending |= error.is_connect() || error.is_request();
        }
        if let Some(error) = error.downcast_ref::<hyper::Error>()
            && (error.is_closed() || error.is_incomplete_message())
        {
            return true;
        }
        if let Some(error) = error.downcast_ref::<h2::Error>() {
            // RST_STREAM is stream-local, not proof that the connection is bad.
            return !error.is_reset()
                && (error.is_io()
                    || error.is_go_away()
                    || matches!(
                        error.reason(),
                        Some(
                            h2::Reason::PROTOCOL_ERROR
                                | h2::Reason::INTERNAL_ERROR
                                | h2::Reason::FRAME_SIZE_ERROR
                                | h2::Reason::COMPRESSION_ERROR
                                | h2::Reason::SETTINGS_TIMEOUT
                                | h2::Reason::HTTP_1_1_REQUIRED
                        )
                    ));
        }
        if (sending || during_http2)
            && let Some(error) = error.downcast_ref::<io::Error>()
            && matches!(
                error.kind(),
                io::ErrorKind::ConnectionReset
                    | io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::BrokenPipe
                    | io::ErrorKind::UnexpectedEof
            )
        {
            return true;
        }
        current = error.source();
    }
    false
}

#[cfg(test)]
mod tests;

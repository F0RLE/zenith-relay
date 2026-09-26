//! Per-provider-HTTP admission, independent of scheduled jobs and inference leases.
//! Only sanitized origins and bounded waiter metadata are retained here.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tokio::{sync::Notify, time::Instant};
use url::Url;

#[derive(Clone, Copy, Debug)]
pub struct HttpLimits {
    pub concurrent: usize,
    pub per_origin: usize,
    pub reserved_auth: usize,
    pub max_waiters: usize,
    pub max_wait: Duration,
}

impl Default for HttpLimits {
    fn default() -> Self {
        Self {
            concurrent: 8,
            per_origin: 3,
            reserved_auth: 1,
            max_waiters: 128,
            max_wait: Duration::from_secs(30),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpClass {
    Auth,
    Ordinary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpAdmissionError {
    InvalidOrigin,
    Full,
    TimedOut,
}

/// The transport error is deliberately not retained or rendered: reqwest may
/// include a user-supplied URL (including a secret query) in its error text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpSendError {
    Admission(HttpAdmissionError),
    Stale,
    Transport { timeout: bool },
}

impl HttpSendError {
    pub fn is_timeout(self) -> bool {
        matches!(self, Self::Transport { timeout: true })
    }
}

/// One gate per provider-facing process. Desktop and Relay Server run in
/// separate processes; every management path, including login/import paths
/// outside the periodic scheduler, joins the same budget within its process.
pub fn management_http_gate() -> Arc<ManagementHttpGate> {
    static GATE: OnceLock<Arc<ManagementHttpGate>> = OnceLock::new();
    GATE.get_or_init(|| ManagementHttpGate::new(HttpLimits::default()).expect("valid HTTP limits"))
        .clone()
}

/// A revision/ownership fence checked immediately before every physical send.
/// No account ID, credential, URL path or provider payload is retained by the
/// gate itself. Owners use this for requests whose configuration may change
/// while an admission waiter is pending.
#[derive(Clone, Default)]
pub struct ManagementHttpScope {
    current: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

impl ManagementHttpScope {
    pub fn checked(check: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self {
            current: Some(Arc::new(check)),
        }
    }

    pub async fn send(
        &self,
        client: &reqwest::Client,
        request: reqwest::RequestBuilder,
        class: HttpClass,
    ) -> Result<(reqwest::Response, HttpPermit), HttpSendError> {
        management_http_gate()
            .send_if_current(client, request, class, || {
                self.current.as_ref().is_none_or(|check| check())
            })
            .await
    }
}

struct Waiter {
    id: u64,
    origin: String,
    class: HttpClass,
}

#[derive(Default)]
struct State {
    next_id: u64,
    total: usize,
    ordinary: usize,
    origins: BTreeMap<String, (usize, usize)>,
    waiters: VecDeque<Waiter>,
}

/// Each host owns one instance for all its provider management HTTP calls.
pub struct ManagementHttpGate {
    limits: HttpLimits,
    state: Mutex<State>,
    changed: Notify,
}

pub struct HttpPermit {
    gate: Arc<ManagementHttpGate>,
    origin: String,
    class: HttpClass,
}

struct Waiting {
    gate: Arc<ManagementHttpGate>,
    id: u64,
    active: bool,
}

impl ManagementHttpGate {
    pub fn new(limits: HttpLimits) -> Result<Arc<Self>, &'static str> {
        if limits.concurrent <= limits.reserved_auth
            || limits.per_origin <= limits.reserved_auth
            || limits.per_origin > limits.concurrent
            || limits.concurrent > 1_024
            || limits.max_waiters == 0
            || limits.max_waiters > 1_024
            || limits.max_wait.is_zero()
            || limits.max_wait > Duration::from_secs(120)
        {
            return Err("invalid management HTTP limits");
        }
        Ok(Arc::new(Self {
            limits,
            state: Mutex::new(State::default()),
            changed: Notify::new(),
        }))
    }

    pub async fn acquire(
        self: &Arc<Self>,
        url: &Url,
        class: HttpClass,
    ) -> Result<HttpPermit, HttpAdmissionError> {
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(HttpAdmissionError::InvalidOrigin);
        }
        let origin = url.origin().ascii_serialization();
        if origin.len() > 512 {
            return Err(HttpAdmissionError::InvalidOrigin);
        }
        let deadline = Instant::now() + self.limits.max_wait;
        let mut waiting = None;
        loop {
            // Subscribe before inspecting the state: release/cancel cannot be
            // lost in the gap between checking capacity and sleeping.
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if Instant::now() >= deadline {
                    return Err(HttpAdmissionError::TimedOut);
                }
                let turn = state
                    .waiters
                    .iter()
                    .find(|entry| self.capacity(&state, &entry.origin, entry.class));
                if self.capacity(&state, &origin, class)
                    && turn.is_none_or(|entry| {
                        Some(entry.id) == waiting.as_ref().map(|w: &Waiting| w.id)
                    })
                {
                    if let Some(mut waiter) = waiting.take() {
                        state.waiters.retain(|entry| entry.id != waiter.id);
                        waiter.active = false;
                    }
                    state.total += 1;
                    if class == HttpClass::Ordinary {
                        state.ordinary += 1;
                    }
                    let entry = state.origins.entry(origin.clone()).or_default();
                    entry.0 += 1;
                    if class == HttpClass::Ordinary {
                        entry.1 += 1;
                    }
                    drop(state);
                    self.changed.notify_waiters();
                    return Ok(HttpPermit {
                        gate: self.clone(),
                        origin,
                        class,
                    });
                }
                if waiting.is_none() {
                    // An ordinary burst must not consume the last waiter
                    // slot needed by login or token recovery. Reserving only
                    // active permits would still reject Auth at a full queue.
                    if state.waiters.len() >= self.limits.max_waiters
                        || (class == HttpClass::Ordinary
                            && state.waiters.len()
                                >= self
                                    .limits
                                    .max_waiters
                                    .saturating_sub(self.limits.reserved_auth))
                    {
                        return Err(HttpAdmissionError::Full);
                    }
                    state.next_id = state
                        .next_id
                        .checked_add(1)
                        .ok_or(HttpAdmissionError::Full)?;
                    let id = state.next_id;
                    state.waiters.push_back(Waiter {
                        id,
                        origin: origin.clone(),
                        class,
                    });
                    waiting = Some(Waiting {
                        gate: self.clone(),
                        id,
                        active: true,
                    });
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Err(HttpAdmissionError::TimedOut);
            }
        }
    }

    /// Acquire for each actual HTTP attempt. The caller must keep the returned
    /// permit until the response body has been fully read or discarded. A 401
    /// retry, auth recovery, or second endpoint gets a separate permit.
    pub async fn send(
        self: &Arc<Self>,
        client: &reqwest::Client,
        request: reqwest::RequestBuilder,
        class: HttpClass,
    ) -> Result<(reqwest::Response, HttpPermit), HttpSendError> {
        self.send_if_current(client, request, class, || true).await
    }

    pub async fn send_if_current(
        self: &Arc<Self>,
        client: &reqwest::Client,
        request: reqwest::RequestBuilder,
        class: HttpClass,
        current: impl Fn() -> bool,
    ) -> Result<(reqwest::Response, HttpPermit), HttpSendError> {
        if !current() {
            return Err(HttpSendError::Stale);
        }
        let request = request
            .build()
            .map_err(|_| HttpSendError::Transport { timeout: false })?;
        let permit = self
            .acquire(request.url(), class)
            .await
            .map_err(HttpSendError::Admission)?;
        if !current() {
            return Err(HttpSendError::Stale);
        }
        let response = client
            .execute(request)
            .await
            .map_err(|error| HttpSendError::Transport {
                timeout: error.is_timeout(),
            })?;
        Ok((response, permit))
    }

    fn capacity(&self, state: &State, origin: &str, class: HttpClass) -> bool {
        let (total, ordinary) = state.origins.get(origin).copied().unwrap_or_default();
        state.total < self.limits.concurrent
            && total < self.limits.per_origin
            && (class == HttpClass::Auth
                || (state.ordinary < self.limits.concurrent - self.limits.reserved_auth
                    && ordinary < self.limits.per_origin - self.limits.reserved_auth))
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.waiters.retain(|entry| entry.id != self.id);
        drop(state);
        self.gate.changed.notify_waiters();
    }
}

impl Drop for HttpPermit {
    fn drop(&mut self) {
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.total -= 1;
        if self.class == HttpClass::Ordinary {
            state.ordinary -= 1;
        }
        let entry = state
            .origins
            .get_mut(&self.origin)
            .expect("permit origin is active");
        entry.0 -= 1;
        if self.class == HttpClass::Ordinary {
            entry.1 -= 1;
        }
        if entry.0 == 0 {
            state.origins.remove(&self.origin);
        }
        drop(state);
        self.gate.changed.notify_waiters();
    }
}

#[cfg(test)]
mod tests;

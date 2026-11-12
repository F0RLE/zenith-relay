//! Same-grant Responses continuation behind the existing HTTP/SSE executor.
use super::{AuthorizationDispatch, AuthorizedRequestError, GatewayRuntime, PreparedAuthorization};
use crate::gateway::websocket::transport::{self, UpstreamWebSocket};
use crate::usage::{ConnectionResetReason, RequestPayloadDiagnostics, TransportMode};
use futures_util::{FutureExt, SinkExt, StreamExt};
use reqwest::{Method, Request, Response, StatusCode, header};
use reqwest_websocket::Message;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

mod idle;
mod planner;
mod stream;
#[cfg(test)]
mod tests;

const MAX_SESSIONS: usize = 32;
const IDLE_TTL: Duration = Duration::from_secs(30 * 60);
const MAX_AGE: Duration = Duration::from_secs(55 * 60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(super) struct SocketStore {
    lanes: Mutex<HashMap<planner::Fingerprint, Arc<AsyncMutex<Lane>>>>,
}

struct Lane {
    idle: Option<idle::IdleSocket>,
    touched: Instant,
    authority: Option<SocketAuthority>,
    reset_reason: Option<ConnectionResetReason>,
}

struct IdleConnection {
    socket: UpstreamWebSocket,
    opened: Instant,
    checkpoint: Option<planner::Checkpoint>,
}

#[derive(Eq, PartialEq)]
struct SocketAuthority {
    credential: [u8; 32],
    incarnation: super::AuthorizationIncarnation,
}

impl Lane {
    fn new() -> Self {
        Self {
            idle: None,
            touched: Instant::now(),
            authority: None,
            reset_reason: Some(ConnectionResetReason::NewSession),
        }
    }

    fn bind(&mut self, authority: SocketAuthority) {
        if self.authority.as_ref() != Some(&authority) {
            self.idle = None;
            if self.authority.is_some() {
                self.reset_reason = Some(ConnectionResetReason::AuthorizationChanged);
            }
            self.authority = Some(authority);
        }
    }
}

pub(crate) enum PreparedSocket {
    NotApplicable,
    Rejected(Response),
    Ready(Box<PendingSocket>),
}

pub(crate) struct PendingSocket {
    lane: OwnedMutexGuard<Lane>,
    connection: IdleConnection,
    plan: planner::Plan,
    response_id: Option<String>,
    completed_output: crate::gateway::NativeReplayCapture,
    url: url::Url,
    diagnostics: SocketDiagnostics,
    pub(super) payload_diagnostics: Option<RequestPayloadDiagnostics>,
}

#[derive(Clone)]
pub(crate) struct SocketDiagnostics {
    pub(crate) reused: bool,
    pub(crate) handshake_ms: Option<u64>,
    pub(crate) reset_reason: Option<ConnectionResetReason>,
}

impl SocketStore {
    fn acquire_or_isolate(&self, key: Option<planner::Fingerprint>) -> OwnedMutexGuard<Lane> {
        key.and_then(|key| self.acquire(key)).unwrap_or_else(|| {
            let mut lane = Lane::new();
            lane.reset_reason = Some(if key.is_some() {
                ConnectionResetReason::SessionUnavailable
            } else {
                ConnectionResetReason::MissingContext
            });
            Arc::new(AsyncMutex::new(lane))
                .try_lock_owned()
                .expect("new lane")
        })
    }

    fn acquire(&self, key: planner::Fingerprint) -> Option<OwnedMutexGuard<Lane>> {
        let mut lanes = crate::poison::mutex(&self.lanes);
        let expired = lanes.get(&key).is_some_and(|lane| {
            lane.try_lock()
                .is_ok_and(|lane| lane.touched.elapsed() >= IDLE_TTL)
        });
        lanes.retain(|_, lane| {
            lane.try_lock()
                .map_or(true, |lane| lane.touched.elapsed() < IDLE_TTL)
        });
        if !lanes.contains_key(&key) && lanes.len() >= MAX_SESSIONS {
            // Reclaim only an unlocked lane. Busy owners keep their socket and
            // checkpoint; new sessions must not displace an in-flight turn.
            let oldest = lanes
                .iter()
                .filter_map(|(key, lane)| lane.try_lock().ok().map(|lane| (*key, lane.touched)))
                .min_by_key(|(_, touched)| *touched)
                .map(|(key, _)| key)?;
            lanes.remove(&oldest);
        }
        lanes
            .entry(key)
            .or_insert_with(|| {
                let mut lane = Lane::new();
                if expired {
                    lane.reset_reason = Some(ConnectionResetReason::SessionExpired);
                }
                Arc::new(AsyncMutex::new(lane))
            })
            .clone()
            .try_lock_owned()
            .ok()
    }
}

impl GatewayRuntime {
    pub(super) async fn prepare_siwc_socket(
        &self,
        candidate: &str,
        request: &Request,
        prepared: &PreparedAuthorization,
        dispatch: AuthorizationDispatch<'_>,
        diagnostics_mode: Option<TransportMode>,
    ) -> Result<PreparedSocket, AuthorizedRequestError> {
        if !self.is_siwc_account(candidate)
            || !requires_text_socket(request)
            || dispatch
                .budget
                .is_some_and(|budget| budget.is_image_generation())
        {
            return Ok(PreparedSocket::NotApplicable);
        }
        if dispatch.budget.is_none() || dispatch.lease.is_none() {
            return Err(required(
                StatusCode::SERVICE_UNAVAILABLE,
                "SIWC WebSocket generation requires an active dispatch lease and budget",
            ));
        }
        if !eligible_endpoint(request) {
            return Err(required(
                StatusCode::BAD_REQUEST,
                "SIWC generation requires the public Responses WebSocket endpoint",
            ));
        }
        let payload = request
            .body()
            .and_then(reqwest::Body::as_bytes)
            .and_then(planner::parse)
            .ok_or_else(|| {
                required(
                    StatusCode::BAD_REQUEST,
                    "The prepared SIWC request is incompatible with Responses WebSocket generation",
                )
            })?;
        let key = dispatch.usage_event.and_then(|(event, _)| {
            let context = event
                .client_context_id
                .as_deref()
                .filter(|s| !s.is_empty())?;
            Some(session_key(
                &event.local_key_id,
                candidate,
                context,
                request.url(),
            ))
        });
        // Context-free and overlapping turns use isolated full-history sockets.
        let mut lane = self.session_state.siwc_sockets.acquire_or_isolate(key);
        lane.bind(SocketAuthority {
            credential: prepared.credential_fingerprint(),
            incarnation: prepared.incarnation(),
        });
        let mut connection = match lane.idle.take() {
            Some(idle) => match idle.resume().await {
                Ok(connection) => Some(connection),
                Err(reason) => {
                    lane.reset_reason = Some(reason);
                    None
                }
            },
            None => None,
        };
        if connection
            .as_ref()
            .is_some_and(|connection| connection.opened.elapsed() >= MAX_AGE)
        {
            connection = None;
            lane.reset_reason = Some(ConnectionResetReason::ConnectionExpired);
        }
        if let Some(existing) = connection.as_mut()
            && let Err(reason) = ready_for_reuse(&mut existing.socket).await
        {
            connection = None;
            lane.reset_reason = Some(reason);
        }
        let reused = connection.is_some();
        let reset_reason = if reused { None } else { lane.reset_reason };
        let mut handshake_ms = None;
        if connection.is_none() {
            let budget = dispatch.budget.expect("checked budget");
            budget.with_budget(|budget| {
                let _guard = prepared
                    .dispatch_guard(self, candidate)
                    .ok_or(AuthorizedRequestError::NotReplayable)?;
                if !budget.can_dispatch() {
                    return Err(AuthorizedRequestError::DispatchBudgetExhausted);
                }
                budget
                    .start_wire_attempt()
                    .ok_or(AuthorizedRequestError::DispatchBudgetExhausted)?;
                Ok::<_, AuthorizedRequestError>(())
            })?;
            let mut headers = request.headers().clone();
            headers.remove(header::CONTENT_LENGTH);
            headers.remove(header::CONTENT_TYPE);
            headers.remove(header::ACCEPT);
            let started = Instant::now();
            let opened = tokio::time::timeout(
                CONNECT_TIMEOUT,
                transport::upgrade(
                    self.websocket_client(candidate)
                        .get(request.url().clone())
                        .headers(headers),
                ),
            )
            .await;
            handshake_ms = Some(started.elapsed().as_millis() as u64);
            let upgrade = match opened {
                Ok(Ok(upgrade)) => upgrade,
                _ => {
                    return Err(required(
                        StatusCode::BAD_GATEWAY,
                        "SIWC WebSocket connection failed before generation",
                    ));
                }
            };
            if upgrade.status() != StatusCode::SWITCHING_PROTOCOLS {
                let status = upgrade.status();
                return if status.is_client_error() || status.is_server_error() {
                    Ok(PreparedSocket::Rejected(upgrade.into_inner()))
                } else {
                    Err(required(
                        StatusCode::BAD_GATEWAY,
                        "SIWC provider did not accept the required WebSocket upgrade",
                    ))
                };
            }
            let Ok(Ok(socket)) =
                tokio::time::timeout(CONNECT_TIMEOUT, upgrade.into_websocket()).await
            else {
                return Err(required(
                    StatusCode::BAD_GATEWAY,
                    "SIWC WebSocket upgrade failed before generation",
                ));
            };
            connection = Some(IdleConnection {
                socket,
                opened: Instant::now(),
                checkpoint: None,
            });
        }
        let connection = connection.expect("connected socket");
        let plan = payload
            .with_diagnostics(diagnostics_mode.is_some())
            .plan(connection.checkpoint.as_ref());
        let payload_diagnostics = diagnostics_mode.map(|mode| {
            let mut diagnostics =
                RequestPayloadDiagnostics::identity(plan.payload.as_bytes(), mode);
            diagnostics.full_request_reason = plan.full_request_reason;
            diagnostics.socket_output_mismatch = plan.output_mismatch.clone();
            diagnostics
        });
        lane.touched = Instant::now();
        lane.reset_reason = Some(ConnectionResetReason::ResponseInterrupted);
        Ok(PreparedSocket::Ready(Box::new(PendingSocket {
            lane,
            connection,
            plan,
            response_id: None,
            completed_output: crate::gateway::NativeReplayCapture::default(),
            url: request.url().clone(),
            diagnostics: SocketDiagnostics {
                reused,
                handshake_ms,
                reset_reason,
            },
            payload_diagnostics,
        })))
    }
}

fn requires_text_socket(request: &Request) -> bool {
    request.method() == Method::POST && !request.url().path().starts_with("/v1/images/")
}

fn eligible_endpoint(request: &Request) -> bool {
    let url = request.url();
    #[cfg(test)]
    if url.scheme() == "http" && url.host_str() == Some("127.0.0.1") {
        return request.method() == Method::POST
            && matches!(url.path(), "/responses" | "/v1/responses")
            && url.query().is_none()
            && url.username().is_empty()
            && url.password().is_none();
    }
    request.method() == Method::POST
        && url.scheme() == "https"
        && url.host_str() == Some("api.openai.com")
        && url.port_or_known_default() == Some(443)
        && url.path() == "/v1/responses"
        && url.query().is_none()
        && url.username().is_empty()
        && url.password().is_none()
}

fn session_key(
    local_key: &str,
    candidate: &str,
    context: &str,
    url: &url::Url,
) -> planner::Fingerprint {
    planner::fingerprint(&serde_json::json!([
        local_key,
        candidate,
        context,
        url.as_str()
    ]))
}

fn required(status: StatusCode, message: &'static str) -> AuthorizedRequestError {
    AuthorizedRequestError::WebsocketRequired { status, message }
}

async fn ready_for_reuse(socket: &mut UpstreamWebSocket) -> Result<(), ConnectionResetReason> {
    for _ in 0..8 {
        match socket.next().now_or_never() {
            None => return Ok(()),
            Some(Some(Ok(Message::Ping(_) | Message::Pong(_)))) => {
                if tokio::time::timeout(Duration::from_secs(1), socket.flush())
                    .await
                    .map_or(true, |v| v.is_err())
                {
                    return Err(ConnectionResetReason::HeartbeatFailed);
                }
            }
            Some(Some(Ok(Message::Text(_) | Message::Binary(_)))) => {
                return Err(ConnectionResetReason::PendingFrame);
            }
            _ => return Err(ConnectionResetReason::ConnectionClosed),
        }
    }
    Err(ConnectionResetReason::PendingFrame)
}

impl SocketDiagnostics {
    pub(crate) fn observe(&self, transport: &mut crate::usage::TransportDiagnostics) {
        transport.upstream = crate::UsageTransport::Websocket;
        transport.fallback_reason = None;
        transport.http_version = None;
        transport.upstream_http_status = None;
        transport.response_headers_ms = None;
        transport.response_format = Some(crate::usage::TransportResponseFormat::WebsocketFrames);
        transport.connection_reused = Some(self.reused);
        transport.connection_reset_reason = self.reset_reason;
        transport.handshake_ms = self.handshake_ms;
    }
}

impl PendingSocket {
    pub(super) fn observe_pending(&self, event: &mut crate::UsageEvent) {
        if let Some(transport) = event
            .routing
            .as_mut()
            .and_then(|routing| routing.transport.as_mut())
        {
            self.diagnostics.observe(transport);
            transport.request_payload = self.payload_diagnostics.clone();
        }
    }
}

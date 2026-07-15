use super::{
    affinity_session, apply_cooldown, apply_failure_state, apply_status_cooldown, apply_usage,
    emit_usage, has_output_delta, invalid_host, now_ms, unauthorized, usage_event,
    CODEX_RESPONSES_LITE_HEADER, TRANSIENT_COOLDOWN_MS,
};
use crate::runtime::{AuthenticatedKey, CandidateLease, ExecutorPrepareError, ExecutorRoute};
use crate::{GatewayRuntime, UsageEvent, WireApi};
use axum::body::Body;
use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, RETRY_AFTER};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Response, StatusCode};
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{
    CloseCode as UpstreamCloseCode, Message as UpstreamMessage, RequestBuilderExt,
    WebSocket as UpstreamWebSocket,
};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::{interval_at, sleep_until, timeout, Instant as TokioInstant, MissedTickBehavior};

const MAX_WEBSOCKET_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_WEBSOCKET_ERROR_BYTES: usize = 1024 * 1024;
const INITIAL_MESSAGE_TIMEOUT: Duration = Duration::from_secs(30);
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const WEBSOCKET_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const WEBSOCKET_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const RESPONSES_WEBSOCKET_BETA: &str = "responses_websockets=2026-02-06";
const RESPONSES_LITE_METADATA_KEY: &str =
    "ws_request_header_x_openai_internal_codex_responses_lite";
const WEBSOCKET_PROTOCOLS: &[WireApi] = &[WireApi::Responses];

const FORWARDED_HEADERS: &[&str] = &[
    "openai-beta",
    "originator",
    "session-id",
    "session_id",
    "thread-id",
    "traceparent",
    "tracestate",
    "user-agent",
    "version",
    "x-client-request-id",
    "x-codex-beta-features",
    "x-codex-installation-id",
    "x-codex-parent-thread-id",
    "x-codex-turn-metadata",
    "x-codex-turn-state",
    "x-codex-window-id",
    "x-oai-attestation",
    "x-openai-memgen-request",
    "x-openai-subagent",
    "x-responsesapi-include-timing-metrics",
    "x-session-id",
];

pub(super) async fn responses(
    State(runtime): State<Arc<GatewayRuntime>>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response<Body> {
    if !super::valid_local_host(&headers) {
        return invalid_host();
    }
    let Some(key) = runtime.authenticate(headers.get(AUTHORIZATION)) else {
        return unauthorized();
    };

    websocket
        .max_message_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .max_frame_size(MAX_WEBSOCKET_MESSAGE_BYTES)
        .write_buffer_size(64 * 1024)
        .max_write_buffer_size(MAX_WEBSOCKET_MESSAGE_BYTES.saturating_mul(2))
        .on_upgrade(move |socket| handle_connection(socket, runtime, key, headers))
}

async fn handle_connection(
    mut downstream: WebSocket,
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    headers: HeaderMap,
) {
    let request = match read_initial_request(&mut downstream, &runtime, &key, &headers).await {
        Ok(request) => request,
        Err(failure) => {
            send_gateway_error(&mut downstream, &failure).await;
            return;
        }
    };

    let connected = match connect_upstream(&runtime, &key, &headers, request).await {
        Ok(connected) => connected,
        Err(failure) => {
            send_gateway_error(&mut downstream, &failure).await;
            return;
        }
    };

    bridge(downstream, runtime, key, headers, connected).await;
}

async fn read_initial_request(
    downstream: &mut WebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    headers: &HeaderMap,
) -> Result<ClientRequest, GatewayFailure> {
    let deadline = TokioInstant::now() + INITIAL_MESSAGE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(TokioInstant::now());
        if remaining.is_zero() {
            return Err(GatewayFailure::request_timeout());
        }
        let message = timeout(remaining, downstream.recv())
            .await
            .map_err(|_| GatewayFailure::request_timeout())?
            .ok_or_else(GatewayFailure::client_closed)?
            .map_err(|_| GatewayFailure::invalid_request("invalid WebSocket frame"))?;
        match message {
            Message::Text(text) => {
                return ClientRequest::parse(runtime, key, headers, text.as_bytes())
            }
            Message::Binary(bytes) => return ClientRequest::parse(runtime, key, headers, &bytes),
            Message::Ping(payload) => {
                if downstream.send(Message::Pong(payload)).await.is_err() {
                    return Err(GatewayFailure::client_closed());
                }
            }
            Message::Pong(_) => {}
            Message::Close(_) => return Err(GatewayFailure::client_closed()),
        }
    }
}

#[derive(Clone)]
struct ClientRequest {
    value: Value,
    requested_model: String,
    resolved_model: String,
    responses_lite: bool,
    affinity_key: Option<String>,
    session_affinity_key: Option<String>,
}

impl ClientRequest {
    fn parse(
        runtime: &GatewayRuntime,
        key: &AuthenticatedKey,
        headers: &HeaderMap,
        payload: &[u8],
    ) -> Result<Self, GatewayFailure> {
        if payload.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
            return Err(GatewayFailure::invalid_request(
                "WebSocket request is too large",
            ));
        }
        let value: Value = serde_json::from_slice(payload)
            .map_err(|_| GatewayFailure::invalid_request("request must be valid JSON"))?;
        let object = value
            .as_object()
            .ok_or_else(|| GatewayFailure::invalid_request("request must be a JSON object"))?;
        if object
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind != "response.create")
        {
            return Err(GatewayFailure::invalid_request(
                "only response.create messages are supported",
            ));
        }
        let requested_model = object
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .ok_or_else(|| GatewayFailure::invalid_request("model must be a non-empty string"))?
            .to_string();
        if !runtime
            .visible_models(key, WEBSOCKET_PROTOCOLS, now_ms())
            .iter()
            .any(|model| model.eq_ignore_ascii_case(&requested_model))
        {
            return Err(GatewayFailure::model_not_found());
        }
        let resolved_model = runtime
            .resolve_model(key, &requested_model)
            .ok_or_else(GatewayFailure::model_not_found)?;
        let responses_lite = headers.contains_key(CODEX_RESPONSES_LITE_HEADER)
            || metadata_flag(&value, RESPONSES_LITE_METADATA_KEY)
            || runtime.codex_model_uses_responses_lite(&resolved_model);
        let session = websocket_affinity_session(headers, &value);
        let session_affinity_key = runtime.affinity_key(
            &key.id,
            WireApi::Responses,
            &resolved_model,
            session.as_deref(),
        );
        let affinity_key = runtime
            .response_affinity_key(value.get("previous_response_id").and_then(Value::as_str))
            .or_else(|| session_affinity_key.clone());
        Ok(Self {
            value,
            requested_model,
            resolved_model,
            responses_lite,
            affinity_key,
            session_affinity_key,
        })
    }

    fn payload_for(&self, route: &ExecutorRoute) -> Result<String, GatewayFailure> {
        let mut value = self.value.clone();
        let object = value
            .as_object_mut()
            .expect("request object was validated before routing");
        object.insert(
            "type".to_string(),
            Value::String("response.create".to_string()),
        );
        object.insert(
            "model".to_string(),
            Value::String(route.source_model.clone()),
        );
        if route.account_id.is_some() {
            super::normalize_account_request(object, self.responses_lite);
        }
        serde_json::to_string(&value)
            .map_err(|_| GatewayFailure::invalid_request("request could not be serialized"))
    }

    fn has_previous_response_id(&self) -> bool {
        self.value
            .get("previous_response_id")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
    }
}

struct Connected {
    upstream: UpstreamWebSocket,
    first_message: UpstreamMessage,
    candidate_id: String,
    route: ExecutorRoute,
    request: ClientRequest,
    lease: CandidateLease,
    attempt: u16,
    started: Instant,
}

async fn connect_upstream(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    client_headers: &HeaderMap,
    request: ClientRequest,
) -> Result<Connected, GatewayFailure> {
    let mut tried = HashSet::new();
    let mut attempt = 0_u16;
    let mut last_failure = None;

    while usize::from(attempt) < runtime.max_retry_candidates() {
        let selected = runtime.select_and_reserve(
            key,
            &request.resolved_model,
            WEBSOCKET_PROTOCOLS,
            &tried,
            request.affinity_key.as_deref(),
            now_ms(),
        );
        let Some((selected, lease)) = selected else {
            break;
        };
        tried.insert(selected.candidate_id.clone());
        let Some(route) = runtime.executor_route(&selected.candidate_id, &request.resolved_model)
        else {
            continue;
        };
        if route.wire_api != WireApi::Responses {
            continue;
        }
        attempt = attempt.saturating_add(1);
        let started = Instant::now();
        let prepared = match runtime
            .prepare_authorization(&route.candidate_id, now_ms())
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                let failure = GatewayFailure::prepare(error);
                record_connect_failure(
                    runtime, key, &route, &request, attempt, started, &failure, None,
                );
                last_failure = Some(failure);
                continue;
            }
        };
        let payload = request.payload_for(&route)?;
        let headers = upstream_headers(client_headers, prepared, request.responses_lite);
        let upgrade = runtime
            .websocket_client(&route.candidate_id)
            .get(route.upstream_url.clone())
            .headers(headers)
            .upgrade();
        let upgrade = match timeout(UPSTREAM_CONNECT_TIMEOUT, upgrade.send()).await {
            Ok(Ok(upgrade)) => upgrade,
            _ => {
                let failure = GatewayFailure::transport();
                record_connect_failure(
                    runtime, key, &route, &request, attempt, started, &failure, None,
                );
                last_failure = Some(failure);
                continue;
            }
        };
        let status = upgrade.status();
        let response_headers = upgrade.headers().clone();
        if status != StatusCode::SWITCHING_PROTOCOLS {
            let response = upgrade.into_inner();
            let _ = timeout(
                UPSTREAM_CONNECT_TIMEOUT,
                crate::runtime::collect_limited(response, MAX_WEBSOCKET_ERROR_BYTES),
            )
            .await;
            let failure = GatewayFailure::upstream_status(status);
            record_connect_failure(
                runtime,
                key,
                &route,
                &request,
                attempt,
                started,
                &failure,
                Some(&response_headers),
            );
            last_failure = Some(failure);
            continue;
        }
        let mut upstream = match timeout(UPSTREAM_CONNECT_TIMEOUT, upgrade.into_websocket()).await {
            Ok(Ok(upstream)) => upstream,
            _ => {
                let failure = GatewayFailure::transport();
                record_connect_failure(
                    runtime, key, &route, &request, attempt, started, &failure, None,
                );
                last_failure = Some(failure);
                continue;
            }
        };
        if send_request(&mut upstream, payload).await.is_err() {
            let failure = GatewayFailure::transport();
            record_connect_failure(
                runtime, key, &route, &request, attempt, started, &failure, None,
            );
            last_failure = Some(failure);
            continue;
        }
        let first_message = match first_application_message(&mut upstream).await {
            Ok(message) => message,
            Err(failure) => {
                let response_headers = HeaderMap::new();
                record_connect_failure(
                    runtime,
                    key,
                    &route,
                    &request,
                    attempt,
                    started,
                    &failure,
                    Some(&response_headers),
                );
                last_failure = Some(failure);
                continue;
            }
        };
        if let Some(terminal) = first_message_terminal(&first_message) {
            if terminal.outcome == Some(false) {
                let status = terminal.status.unwrap_or(StatusCode::BAD_GATEWAY);
                if super::retryable_status(status, request.has_previous_response_id()) {
                    let failure = GatewayFailure::upstream_status(status);
                    record_connect_failure(
                        runtime,
                        key,
                        &route,
                        &request,
                        attempt,
                        started,
                        &failure,
                        Some(&terminal.headers),
                    );
                    last_failure = Some(failure);
                    continue;
                }
            }
        }
        return Ok(Connected {
            upstream,
            first_message,
            candidate_id: route.candidate_id.clone(),
            route,
            request,
            lease,
            attempt,
            started,
        });
    }

    if let Some(retry_at_ms) = runtime.earliest_retry_at(
        key,
        &request.resolved_model,
        WEBSOCKET_PROTOCOLS,
        &tried,
        now_ms(),
    ) {
        return Err(GatewayFailure::cooldown(retry_at_ms));
    }
    Err(last_failure.unwrap_or_else(GatewayFailure::unavailable))
}

async fn first_application_message(
    upstream: &mut UpstreamWebSocket,
) -> Result<UpstreamMessage, GatewayFailure> {
    let deadline = TokioInstant::now() + WEBSOCKET_IDLE_TIMEOUT;
    let mut heartbeat = interval_at(
        TokioInstant::now() + WEBSOCKET_HEARTBEAT_INTERVAL,
        WEBSOCKET_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = sleep_until(deadline) => return Err(GatewayFailure::idle_timeout()),
            _ = heartbeat.tick() => {
                upstream
                    .send(UpstreamMessage::Ping(Default::default()))
                    .await
                    .map_err(|_| GatewayFailure::transport())?;
            }
            message = upstream.next() => {
                match message {
                    Some(Ok(message @ (UpstreamMessage::Text(_) | UpstreamMessage::Binary(_)))) => {
                        return Ok(message);
                    }
                    Some(Ok(UpstreamMessage::Ping(payload))) => {
                        upstream
                            .send(UpstreamMessage::Pong(payload))
                            .await
                            .map_err(|_| GatewayFailure::transport())?;
                    }
                    Some(Ok(UpstreamMessage::Pong(_))) => {}
                    Some(Ok(UpstreamMessage::Close { .. })) | None => {
                        return Err(GatewayFailure::closed());
                    }
                    Some(Err(_)) => return Err(GatewayFailure::transport()),
                }
            }
        }
    }
}

fn first_message_terminal(message: &UpstreamMessage) -> Option<EventTerminal> {
    let payload = match message {
        UpstreamMessage::Text(text) => text.as_bytes(),
        UpstreamMessage::Binary(bytes) => bytes.as_ref(),
        _ => return None,
    };
    let value = serde_json::from_slice::<Value>(payload).ok()?;
    Some(event_terminal(&value))
}

#[allow(clippy::too_many_arguments)]
fn record_connect_failure(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    route: &ExecutorRoute,
    request: &ClientRequest,
    attempt: u16,
    started: Instant,
    failure: &GatewayFailure,
    headers: Option<&HeaderMap>,
) {
    let state = match headers {
        Some(headers) => apply_status_cooldown(
            runtime,
            &route.candidate_id,
            &route.source_model,
            failure.status,
            headers,
        ),
        None => apply_cooldown(runtime, &route.candidate_id, "*", failure.cooldown_ms),
    };
    let mut event = usage_event(
        &super::request_id(),
        attempt,
        &key.id,
        route,
        &request.requested_model,
        false,
        failure.status.as_u16(),
        Some(failure.category.to_string()),
        started.elapsed().as_millis() as u64,
    );
    apply_failure_state(&mut event, state);
    emit_usage(runtime, event);
}

fn upstream_headers(
    client_headers: &HeaderMap,
    prepared: crate::runtime::PreparedAuthorization,
    responses_lite: bool,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for &name in FORWARDED_HEADERS {
        if let Some(value) = client_headers.get(name) {
            headers.insert(HeaderName::from_static(name), value.clone());
        }
    }
    headers.insert(AUTHORIZATION, prepared.authorization);
    if let Some(account_id) = prepared.chatgpt_account_id {
        headers.insert(HeaderName::from_static("chatgpt-account-id"), account_id);
    }
    if !headers.contains_key("originator") {
        if let Some(originator) = prepared.originator {
            headers.insert(HeaderName::from_static("originator"), originator);
        }
    }
    if responses_lite {
        headers.insert(
            HeaderName::from_static(CODEX_RESPONSES_LITE_HEADER),
            HeaderValue::from_static("true"),
        );
    }
    ensure_websocket_beta(&mut headers);
    headers
}

fn ensure_websocket_beta(headers: &mut HeaderMap) {
    let name = HeaderName::from_static("openai-beta");
    let present = headers
        .get_all(&name)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| value.contains("responses_websockets="));
    if !present {
        headers.append(name, HeaderValue::from_static(RESPONSES_WEBSOCKET_BETA));
    }
}

async fn send_request(
    upstream: &mut UpstreamWebSocket,
    payload: String,
) -> Result<(), GatewayFailure> {
    let send = async {
        upstream.send(UpstreamMessage::Text(payload)).await?;
        upstream.flush().await
    };
    match timeout(UPSTREAM_CONNECT_TIMEOUT, send).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(GatewayFailure::transport()),
    }
}

struct InFlight {
    route: ExecutorRoute,
    event: UsageEvent,
    started: Instant,
    affinity_key: Option<String>,
    response_id: Option<String>,
}

struct BridgeState {
    candidate_id: String,
    lease: Option<CandidateLease>,
    in_flight: Option<InFlight>,
}

async fn bridge(
    mut downstream: WebSocket,
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    headers: HeaderMap,
    connected: Connected,
) {
    let mut upstream = connected.upstream;
    let initial_event = usage_event(
        &super::request_id(),
        connected.attempt,
        &key.id,
        &connected.route,
        &connected.request.requested_model,
        true,
        StatusCode::OK.as_u16(),
        None,
        0,
    );
    let mut state = BridgeState {
        candidate_id: connected.candidate_id,
        lease: Some(connected.lease),
        in_flight: Some(InFlight {
            route: connected.route,
            event: initial_event,
            started: connected.started,
            affinity_key: connected.request.session_affinity_key,
            response_id: None,
        }),
    };
    if !handle_upstream_message(
        &mut downstream,
        &runtime,
        &mut state,
        connected.first_message,
    )
    .await
    {
        return;
    }
    let mut last_activity = TokioInstant::now();
    let mut heartbeat = interval_at(
        TokioInstant::now() + WEBSOCKET_HEARTBEAT_INTERVAL,
        WEBSOCKET_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        let idle_deadline = last_activity + WEBSOCKET_IDLE_TIMEOUT;
        tokio::select! {
            _ = sleep_until(idle_deadline) => {
                finish_incomplete(&runtime, &mut state, "websocket_idle_timeout");
                let _ = downstream.send(Message::Close(Some(CloseFrame {
                    code: close_code::AWAY,
                    reason: "idle timeout".into(),
                }))).await;
                break;
            }
            _ = heartbeat.tick() => {
                if upstream.send(UpstreamMessage::Ping(Default::default())).await.is_err() {
                    finish_incomplete(&runtime, &mut state, "upstream_websocket");
                    break;
                }
            }
            message = downstream.recv() => {
                last_activity = TokioInstant::now();
                let Some(message) = message else {
                    finish_incomplete(&runtime, &mut state, "client_cancelled");
                    break;
                };
                let Ok(message) = message else {
                    finish_incomplete(&runtime, &mut state, "client_websocket");
                    break;
                };
                match handle_downstream_message(
                    &mut downstream,
                    &mut upstream,
                    &runtime,
                    &key,
                    &headers,
                    &mut state,
                    message,
                ).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(failure) => {
                        finish_incomplete(&runtime, &mut state, failure.category);
                        send_gateway_error(&mut downstream, &failure).await;
                        break;
                    }
                }
            }
            message = upstream.next() => {
                last_activity = TokioInstant::now();
                let Some(message) = message else {
                    finish_incomplete(&runtime, &mut state, "upstream_websocket_closed");
                    break;
                };
                let Ok(message) = message else {
                    finish_incomplete(&runtime, &mut state, "upstream_websocket");
                    break;
                };
                if !handle_upstream_message(&mut downstream, &runtime, &mut state, message).await {
                    break;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_downstream_message(
    downstream: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    headers: &HeaderMap,
    state: &mut BridgeState,
    message: Message,
) -> Result<bool, GatewayFailure> {
    match message {
        Message::Text(text) => {
            return start_next_request(
                downstream,
                upstream,
                runtime,
                key,
                headers,
                state,
                text.as_bytes(),
            )
            .await;
        }
        Message::Binary(bytes) => {
            return start_next_request(downstream, upstream, runtime, key, headers, state, &bytes)
                .await;
        }
        Message::Ping(payload) => {
            upstream
                .send(UpstreamMessage::Ping(payload))
                .await
                .map_err(|_| GatewayFailure::transport())?;
        }
        Message::Pong(payload) => {
            upstream
                .send(UpstreamMessage::Pong(payload))
                .await
                .map_err(|_| GatewayFailure::transport())?;
        }
        Message::Close(frame) => {
            let (code, reason) = frame
                .map(|frame| {
                    (
                        UpstreamCloseCode::from(frame.code),
                        frame.reason.to_string(),
                    )
                })
                .unwrap_or((UpstreamCloseCode::Normal, String::new()));
            let _ = upstream.send(UpstreamMessage::Close { code, reason }).await;
            let _ = downstream.send(Message::Close(None)).await;
            finish_incomplete(runtime, state, "client_cancelled");
            return Ok(false);
        }
    }
    Ok(true)
}

async fn start_next_request(
    downstream: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    headers: &HeaderMap,
    state: &mut BridgeState,
    payload: &[u8],
) -> Result<bool, GatewayFailure> {
    if state.in_flight.is_some() {
        return Err(GatewayFailure::invalid_request(
            "a response is already in progress",
        ));
    }
    let request = ClientRequest::parse(runtime, key, headers, payload)?;
    if !request.has_previous_response_id() {
        let connected = connect_upstream(runtime, key, headers, request).await?;
        let Connected {
            upstream: next_upstream,
            first_message,
            candidate_id,
            route,
            request,
            lease,
            attempt,
            started,
        } = connected;
        let event = usage_event(
            &super::request_id(),
            attempt,
            &key.id,
            &route,
            &request.requested_model,
            true,
            StatusCode::OK.as_u16(),
            None,
            0,
        );
        let _ = upstream
            .send(UpstreamMessage::Close {
                code: UpstreamCloseCode::Normal,
                reason: String::new(),
            })
            .await;
        *upstream = next_upstream;
        state.candidate_id = candidate_id;
        state.lease = Some(lease);
        state.in_flight = Some(InFlight {
            route,
            event,
            started,
            affinity_key: request.session_affinity_key,
            response_id: None,
        });
        return Ok(handle_upstream_message(downstream, runtime, state, first_message).await);
    }

    let route = runtime
        .executor_route(&state.candidate_id, &request.resolved_model)
        .filter(|route| route.wire_api == WireApi::Responses)
        .ok_or_else(GatewayFailure::model_not_found)?;
    let payload = request.payload_for(&route)?;
    let lease = runtime
        .reserve_candidate(&state.candidate_id)
        .ok_or_else(GatewayFailure::unavailable)?;
    let started = Instant::now();
    send_request(upstream, payload).await?;
    state.lease = Some(lease);
    state.in_flight = Some(InFlight {
        event: usage_event(
            &super::request_id(),
            1,
            &key.id,
            &route,
            &request.requested_model,
            true,
            StatusCode::OK.as_u16(),
            None,
            0,
        ),
        route,
        started,
        affinity_key: request.session_affinity_key,
        response_id: None,
    });
    Ok(true)
}

async fn handle_upstream_message(
    downstream: &mut WebSocket,
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    message: UpstreamMessage,
) -> bool {
    match message {
        UpstreamMessage::Text(text) => {
            if text.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
                finish_incomplete(runtime, state, "stream_event_too_large");
                return false;
            }
            let terminal = inspect_upstream_event(text.as_bytes(), state);
            if downstream.send(Message::Text(text.into())).await.is_err() {
                finish_incomplete(runtime, state, "client_cancelled");
                return false;
            }
            finish_terminal(runtime, state, terminal)
        }
        UpstreamMessage::Binary(bytes) => {
            if bytes.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
                finish_incomplete(runtime, state, "stream_event_too_large");
                return false;
            }
            let terminal = inspect_upstream_event(&bytes, state);
            if downstream.send(Message::Binary(bytes)).await.is_err() {
                finish_incomplete(runtime, state, "client_cancelled");
                return false;
            }
            finish_terminal(runtime, state, terminal)
        }
        UpstreamMessage::Ping(payload) => downstream.send(Message::Ping(payload)).await.is_ok(),
        UpstreamMessage::Pong(payload) => downstream.send(Message::Pong(payload)).await.is_ok(),
        UpstreamMessage::Close { code, reason } => {
            finish_incomplete(runtime, state, "upstream_websocket_closed");
            let _ = downstream
                .send(Message::Close(Some(CloseFrame {
                    code: u16::from(code),
                    reason: reason.into(),
                })))
                .await;
            false
        }
    }
}

#[derive(Default)]
struct EventTerminal {
    outcome: Option<bool>,
    status: Option<StatusCode>,
    headers: HeaderMap,
}

fn inspect_upstream_event(payload: &[u8], state: &mut BridgeState) -> EventTerminal {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return EventTerminal::default();
    };
    let event_type = value.get("type").and_then(Value::as_str);
    if let Some(in_flight) = state.in_flight.as_mut() {
        if has_output_delta(&value, event_type) && in_flight.event.ttft_ms.is_none() {
            in_flight.event.ttft_ms = Some(in_flight.started.elapsed().as_millis() as u64);
        }
        if let Some(usage) = super::find_usage(&value) {
            apply_usage(&mut in_flight.event, usage);
        }
        if let Some(response_id) = super::response_id(&value) {
            in_flight.response_id = Some(response_id.to_string());
        }
    }
    event_terminal(&value)
}

fn event_terminal(value: &Value) -> EventTerminal {
    let outcome = match value.get("type").and_then(Value::as_str) {
        Some("response.completed" | "response.done") => Some(true),
        Some("response.failed" | "response.incomplete" | "error") => Some(false),
        _ => None,
    };
    EventTerminal {
        outcome,
        status: websocket_status(value),
        headers: websocket_retry_headers(value),
    }
}

fn finish_terminal(
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    terminal: EventTerminal,
) -> bool {
    let Some(success) = terminal.outcome else {
        return true;
    };
    let Some(mut in_flight) = state.in_flight.take() else {
        return true;
    };
    in_flight.event.latency_ms = in_flight.started.elapsed().as_millis() as u64;
    in_flight.event.success = success;
    if success {
        runtime.record_success(
            &in_flight.route.candidate_id,
            &in_flight.route.source_model,
            now_ms(),
        );
        runtime.bind_affinity(
            in_flight.affinity_key.as_deref(),
            &in_flight.route.candidate_id,
            now_ms(),
        );
        runtime.bind_response_affinity(
            in_flight.response_id.as_deref(),
            &in_flight.route.candidate_id,
            now_ms(),
        );
        in_flight.event.consecutive_failures = Some(0);
    } else {
        let status = terminal.status.unwrap_or(StatusCode::BAD_GATEWAY);
        in_flight.event.http_status = status.as_u16();
        in_flight.event.error_category = Some(websocket_error_category(status).to_string());
        let failure_state = apply_status_cooldown(
            runtime,
            &in_flight.route.candidate_id,
            &in_flight.route.source_model,
            status,
            &terminal.headers,
        );
        apply_failure_state(&mut in_flight.event, failure_state);
    }
    emit_usage(runtime, in_flight.event);
    state.lease.take();
    success
}

fn finish_incomplete(runtime: &GatewayRuntime, state: &mut BridgeState, category: &str) {
    let Some(mut in_flight) = state.in_flight.take() else {
        state.lease.take();
        return;
    };
    in_flight.event.success = false;
    in_flight.event.error_category = Some(category.to_string());
    in_flight.event.latency_ms = in_flight.started.elapsed().as_millis() as u64;
    if incomplete_requires_cooldown(category) {
        let failure_state = apply_cooldown(
            runtime,
            &in_flight.route.candidate_id,
            &in_flight.route.source_model,
            TRANSIENT_COOLDOWN_MS,
        );
        apply_failure_state(&mut in_flight.event, failure_state);
    }
    emit_usage(runtime, in_flight.event);
    state.lease.take();
}

fn incomplete_requires_cooldown(category: &str) -> bool {
    matches!(
        category,
        "stream_event_too_large"
            | "upstream_transport"
            | "upstream_websocket"
            | "upstream_websocket_closed"
            | "websocket_idle_timeout"
    )
}

fn websocket_status(value: &Value) -> Option<StatusCode> {
    [
        value.get("status"),
        value.get("status_code"),
        value.pointer("/body/status"),
        value.pointer("/body/status_code"),
        value.pointer("/error/status"),
    ]
    .into_iter()
    .flatten()
    .find_map(|value| {
        value
            .as_u64()
            .and_then(|status| u16::try_from(status).ok())
            .and_then(|status| StatusCode::from_u16(status).ok())
    })
}

fn websocket_retry_headers(value: &Value) -> HeaderMap {
    let mut headers = HeaderMap::new();
    let retry_after = value
        .pointer("/headers/retry-after")
        .or_else(|| value.pointer("/headers/retry_after"))
        .or_else(|| value.pointer("/body/headers/retry-after"))
        .or_else(|| value.pointer("/body/error/resets_in_seconds"))
        .or_else(|| value.pointer("/error/resets_in_seconds"));
    let retry_after = retry_after.and_then(|value| match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    });
    if let Some(value) = retry_after
        .filter(|value| value.len() <= 128)
        .and_then(|value| HeaderValue::from_str(&value).ok())
    {
        headers.insert(RETRY_AFTER, value);
    }
    headers
}

fn websocket_error_category(status: StatusCode) -> &'static str {
    match status {
        StatusCode::UNAUTHORIZED => "upstream_authentication",
        StatusCode::FORBIDDEN => "upstream_forbidden",
        StatusCode::NOT_FOUND => "upstream_model_not_found",
        StatusCode::TOO_MANY_REQUESTS => "upstream_rate_limited",
        _ => "upstream_websocket",
    }
}

fn metadata_flag(value: &Value, key: &str) -> bool {
    value
        .get("client_metadata")
        .and_then(|metadata| metadata.get(key))
        .is_some_and(|value| match value {
            Value::Bool(value) => *value,
            Value::String(value) => value.eq_ignore_ascii_case("true"),
            _ => false,
        })
}

fn websocket_affinity_session(headers: &HeaderMap, request: &Value) -> Option<String> {
    request
        .get("client_metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| {
            ["session_id", "thread_id", "turn_id"]
                .into_iter()
                .find_map(|key| metadata.get(key).and_then(Value::as_str))
        })
        .map(str::to_string)
        .or_else(|| affinity_session(headers, request))
}

async fn send_gateway_error(downstream: &mut WebSocket, failure: &GatewayFailure) {
    let event = json!({
        "type": "error",
        "status": failure.status.as_u16(),
        "error": {
            "type": failure.category,
            "code": failure.category,
            "message": failure.message,
        },
        "retry_at_ms": failure.retry_at_ms,
    });
    let _ = downstream
        .send(Message::Text(event.to_string().into()))
        .await;
    let _ = downstream
        .send(Message::Close(Some(CloseFrame {
            code: if failure.status.is_client_error() {
                close_code::POLICY
            } else {
                close_code::ERROR
            },
            reason: "request failed".into(),
        })))
        .await;
}

struct GatewayFailure {
    status: StatusCode,
    category: &'static str,
    message: &'static str,
    cooldown_ms: u64,
    retry_at_ms: Option<u64>,
}

impl GatewayFailure {
    fn invalid_request(message: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: "invalid_request",
            message,
            cooldown_ms: 0,
            retry_at_ms: None,
        }
    }

    fn model_not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            category: "model_not_found",
            message: "model is not available for this local key",
            cooldown_ms: 0,
            retry_at_ms: None,
        }
    }

    fn request_timeout() -> Self {
        Self {
            status: StatusCode::REQUEST_TIMEOUT,
            category: "request_timeout",
            message: "response.create was not received in time",
            cooldown_ms: 0,
            retry_at_ms: None,
        }
    }

    fn client_closed() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: "client_cancelled",
            message: "client closed the WebSocket connection",
            cooldown_ms: 0,
            retry_at_ms: None,
        }
    }

    fn prepare(error: ExecutorPrepareError) -> Self {
        let failure = super::AttemptFailure::prepare(error);
        Self {
            status: failure.status,
            category: failure.category,
            message: failure.message,
            cooldown_ms: failure.cooldown_ms,
            retry_at_ms: None,
        }
    }

    fn transport() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "upstream_transport",
            message: "upstream WebSocket connection failed",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
            retry_at_ms: None,
        }
    }

    fn closed() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "upstream_websocket_closed",
            message: "upstream WebSocket closed before the first event",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
            retry_at_ms: None,
        }
    }

    fn idle_timeout() -> Self {
        Self {
            status: StatusCode::GATEWAY_TIMEOUT,
            category: "websocket_idle_timeout",
            message: "upstream WebSocket produced no event before the idle timeout",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
            retry_at_ms: None,
        }
    }

    fn upstream_status(status: StatusCode) -> Self {
        Self {
            status,
            category: websocket_error_category(status),
            message: "upstream rejected the WebSocket connection",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
            retry_at_ms: None,
        }
    }

    fn unavailable() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            category: "no_eligible_source",
            message: "no eligible WebSocket source is available",
            cooldown_ms: 0,
            retry_at_ms: None,
        }
    }

    fn cooldown(retry_at_ms: u64) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            category: "all_candidates_cooling_down",
            message: "all eligible sources are cooling down",
            cooldown_ms: 0,
            retry_at_ms: Some(retry_at_ms),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::incomplete_requires_cooldown;

    #[test]
    fn only_upstream_incomplete_failures_cool_the_candidate() {
        assert!(incomplete_requires_cooldown("upstream_websocket_closed"));
        assert!(incomplete_requires_cooldown("websocket_idle_timeout"));
        assert!(!incomplete_requires_cooldown("client_cancelled"));
        assert!(!incomplete_requires_cooldown("invalid_request"));
    }
}

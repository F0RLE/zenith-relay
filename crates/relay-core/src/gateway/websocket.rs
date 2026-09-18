use super::auth::{client_api_forbidden, invalid_host, unauthorized};
use super::errors::{
    apply_cooldown, apply_failure_cooldown_with_hint, apply_failure_state, rate_limit_body_hint,
    CooldownContext, RateLimitBodyHint, TRANSIENT_COOLDOWN_MS,
};
use super::execution::{execute_client_request, AutomaticRecovery, CandidateRetryContext};
use super::now_ms;
use super::request::{
    apply_codex_routing_hint, client_context_fingerprint, codex_client_version,
    forwarded_codex_headers, CODEX_RESPONSES_LITE_HEADER,
};
use super::response::{apply_usage, emit_usage, route_error_origin, usage_event};
use super::streaming::{
    has_output_delta, has_semantic_output, is_compaction_payload, is_empty_responses_incomplete,
    is_known_non_output_event, parse_sse_event, NativeReplayCapture, MAX_SSE_EVENT_BYTES,
};
use super::turn_state::{
    guard_account_request, note_account_response_header, CODEX_TURN_STATE_HEADER,
};
use crate::error_codes;
use crate::protocol::ClientWireApi;
use crate::runtime::{AuthenticatedKey, CandidateLease, ExecutorPrepareError, ExecutorRoute};
use crate::{ErrorOrigin, GatewayRuntime, UsageEvent, WireApi};
use axum::body::Body;
use axum::extract::ws::{close_code, CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode};
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{
    CloseCode as UpstreamCloseCode, Message as UpstreamMessage, Upgrade,
    WebSocket as UpstreamWebSocket,
};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::{interval_at, sleep_until, timeout, Instant as TokioInstant, MissedTickBehavior};

mod events;
mod failure;
mod request;

use events::{
    event_terminal, incomplete_requires_cooldown, incomplete_status, terminal_failure_status,
    EventTerminal, EventTerminalOutcome,
};
use failure::{send_gateway_error, GatewayFailure};
use request::ClientRequest;

const WEBSOCKET_SEMANTIC_TIMEOUT: Duration = Duration::from_secs(600);

const MAX_WEBSOCKET_MESSAGE_BYTES: usize = super::request::MAX_CLIENT_REQUEST_BODY_BYTES;
const MAX_WEBSOCKET_ERROR_BYTES: usize = 1024 * 1024;
const INITIAL_MESSAGE_TIMEOUT: Duration = Duration::from_secs(60);
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
const WEBSOCKET_IDLE_TIMEOUT: Duration = Duration::from_secs(900);
const WEBSOCKET_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const RESPONSES_WEBSOCKET_BETA: &str = "responses_websockets=2026-02-06";
const RESPONSES_LITE_METADATA_KEY: &str =
    "ws_request_header_x_openai_internal_codex_responses_lite";
const WEBSOCKET_PROTOCOLS: &[WireApi] = &[WireApi::Responses];

pub(super) async fn responses(
    State(runtime): State<Arc<GatewayRuntime>>,
    headers: HeaderMap,
    websocket: WebSocketUpgrade,
) -> Response<Body> {
    if !super::auth::valid_local_host(&headers) {
        return invalid_host();
    }
    let Some(key) = runtime.authenticate(headers.get(AUTHORIZATION)) else {
        return unauthorized();
    };
    if !runtime.allows_client_wire_api(&key, ClientWireApi::Responses) {
        return client_api_forbidden();
    }

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
            send_gateway_error(&mut downstream, &failure, None).await;
            return;
        }
    };

    let request_id = request.request_id.clone();
    if let Some(kind) = request.background_kind {
        if !runtime.codex_background_tasks_enabled() {
            runtime.blocked_codex_background_event(
                &request.request_id,
                &key.id,
                &request.requested_model,
                WireApi::Responses,
                kind,
            );
            let payload = serde_json::json!({
                "type": "response.completed",
                "response": {"id": format!("resp_relay_blocked_{}", request.request_id), "object": "response", "status": "completed", "output": [], "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}, "metadata": {"zenith_relay": {"blocked": true, "request_type": kind}}}
            });
            let _ = downstream
                .send(Message::Text(payload.to_string().into()))
                .await;
            let _ = downstream.send(Message::Close(None)).await;
            return;
        }
    }
    let fallback_request = request.clone();
    if !runtime.codex_websockets_enabled() {
        bridge_http_fallback(downstream, runtime, key, headers, fallback_request).await;
        return;
    }
    let connected = match connect_upstream_while_client_connected(
        &mut downstream,
        &runtime,
        &key,
        &headers,
        request,
        true,
        0,
        None,
    )
    .await
    {
        Ok(connected) => connected,
        Err(failure) if failure.category == error_codes::UPSTREAM_WEBSOCKET_UNSUPPORTED => {
            bridge_http_fallback(downstream, runtime, key, headers, fallback_request).await;
            return;
        }
        Err(failure) => {
            send_gateway_error(&mut downstream, &failure, Some(&request_id)).await;
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

/// Keeps the client-facing Responses WebSocket usable when every selected
/// provider only exposes HTTP/SSE. The normal HTTP executor remains the single
/// source of routing, adapters, retries, usage, and quota accounting.
async fn bridge_http_fallback(
    mut downstream: WebSocket,
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    headers: HeaderMap,
    mut request: ClientRequest,
) {
    let mut stream_id = request.stream_id.clone();
    loop {
        let mut client_visible_output = false;
        if let Err(failure) = serve_http_fallback_request(
            &mut downstream,
            runtime.clone(),
            &key,
            &headers,
            &request,
            &mut client_visible_output,
        )
        .await
        {
            if !client_visible_output {
                let request_id = Some(request.request_id.as_str());
                send_gateway_error(&mut downstream, &failure, request_id).await;
            } else if failure.category != "client_closed" {
                let _ = downstream
                    .send(Message::Close(Some(CloseFrame {
                        code: close_code::ERROR,
                        reason: "upstream stream ended after output".into(),
                    })))
                    .await;
            }
            return;
        }

        let next = timeout(WEBSOCKET_IDLE_TIMEOUT, async {
            loop {
                let message = downstream.recv().await?;
                let Ok(message) = message else {
                    return None;
                };
                match message {
                    Message::Text(text) => return Some(Some(text.to_string().into_bytes())),
                    Message::Binary(bytes) => return Some(Some(bytes.to_vec())),
                    Message::Ping(payload) => {
                        if downstream.send(Message::Pong(payload)).await.is_err() {
                            return None;
                        }
                    }
                    Message::Pong(_) => {}
                    Message::Close(frame) => {
                        let _ = downstream.send(Message::Close(frame)).await;
                        return None;
                    }
                }
            }
        })
        .await;
        let Ok(Some(Some(payload))) = next else {
            return;
        };
        let next_request = match ClientRequest::parse(&runtime, &key, &headers, &payload) {
            Ok(request) => request,
            Err(failure) => {
                send_gateway_error(&mut downstream, &failure, None).await;
                return;
            }
        };
        if let Some(next_stream_id) = next_request.stream_id.as_deref() {
            if let Some(expected) = stream_id.as_deref() {
                if expected != next_stream_id {
                    let failure = GatewayFailure::invalid_request(
                        "only one WebSocket stream_id is supported per connection",
                    );
                    send_gateway_error(&mut downstream, &failure, Some(&next_request.request_id))
                        .await;
                    return;
                }
            } else {
                stream_id = Some(next_stream_id.to_string());
            }
        }
        request = next_request;
    }
}

async fn serve_http_fallback_request(
    downstream: &mut WebSocket,
    runtime: Arc<GatewayRuntime>,
    _key: &AuthenticatedKey,
    client_headers: &HeaderMap,
    request: &ClientRequest,
    client_visible_output: &mut bool,
) -> Result<(), GatewayFailure> {
    let mut headers = client_headers.clone();
    for name in [
        "connection",
        "upgrade",
        "sec-websocket-key",
        "sec-websocket-version",
        "sec-websocket-protocol",
        "content-length",
    ] {
        headers.remove(name);
    }
    let http_request = Request::builder()
        .method(Method::POST)
        .uri("/v1/responses")
        .header("host", "localhost")
        .body(Body::from(request.http_payload()?))
        .map_err(|_| GatewayFailure::invalid_request("request could not be serialized"))
        .map(|mut request| {
            *request.headers_mut() = headers;
            request
        })?;
    let response = await_while_client_connected(
        downstream,
        execute_client_request(runtime, http_request, WireApi::Responses),
    )
    .await?;
    // The HTTP executor already knows whether the selected route belongs to an
    // account or an API provider. Preserve that attribution across the
    // WebSocket bridge instead of turning every fallback response into a Relay
    // error merely because the bridge is the component reading it.
    let response_origin = fallback_response_origin(&response);
    if !response.status().is_success() {
        let status = response.status();
        let body = await_while_client_connected(
            downstream,
            axum::body::to_bytes(response.into_body(), MAX_WEBSOCKET_ERROR_BYTES),
        )
        .await?
        .ok();
        return Err(GatewayFailure::upstream_status(
            status,
            body.as_deref(),
            response_origin,
        ));
    }

    let stream_origin = response_origin;

    let mut body = response.into_body().into_data_stream();
    let mut pending = Vec::new();
    while let Some(chunk) = await_while_client_connected(downstream, body.next()).await? {
        let chunk = chunk.map_err(|_| GatewayFailure::transport(stream_origin))?;
        pending.extend_from_slice(&chunk);
        while let Some(end) = crate::protocol::sse_event_end(&pending) {
            if end > MAX_SSE_EVENT_BYTES {
                return Err(GatewayFailure::message_too_large(ErrorOrigin::Relay));
            }
            let event = pending.drain(..end).collect::<Vec<_>>();
            let terminal = parse_sse_event(&event);
            if terminal.has_data && !terminal.valid {
                return Err(GatewayFailure::transport(stream_origin));
            }
            if let Some(payload) = fallback_event_message(&terminal, stream_origin)? {
                *client_visible_output |= payload.semantic_output;
                downstream
                    .send(payload.message)
                    .await
                    .map_err(|_| GatewayFailure::client_closed())?;
            }
            if terminal.outcome.is_some() {
                return Ok(());
            }
        }
        // Complete events have already been drained above. Only an incomplete
        // tail counts against the per-event budget; one transport chunk may
        // contain several complete events whose combined size is larger.
        if pending.len() > MAX_SSE_EVENT_BYTES {
            return Err(GatewayFailure::message_too_large(ErrorOrigin::Relay));
        }
    }
    Err(GatewayFailure::closed(stream_origin))
}

struct FallbackEventMessage {
    message: Message,
    semantic_output: bool,
}

fn fallback_event_message(
    terminal: &super::streaming::TerminalEvent,
    origin: ErrorOrigin,
) -> Result<Option<FallbackEventMessage>, GatewayFailure> {
    if let Some(raw_data) = terminal.raw_data.as_deref() {
        if raw_data.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
            return Err(GatewayFailure::message_too_large(origin));
        }
        let semantic_output = !terminal.is_compaction && semantic_output_payload(raw_data);
        let message = match String::from_utf8(raw_data.to_vec()) {
            Ok(text) => Message::Text(text.into()),
            Err(error) => Message::Binary(error.into_bytes().into()),
        };
        return Ok(Some(FallbackEventMessage {
            message,
            semantic_output,
        }));
    }

    let Some(payload) = terminal.payload.as_ref() else {
        return Ok(None);
    };
    let payload = serde_json::to_vec(payload).map_err(|_| GatewayFailure::transport(origin))?;
    if payload.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
        return Err(GatewayFailure::message_too_large(origin));
    }
    let semantic_output = semantic_output_payload(&payload);
    let text = String::from_utf8(payload).map_err(|_| GatewayFailure::transport(origin))?;
    Ok(Some(FallbackEventMessage {
        message: Message::Text(text.into()),
        semantic_output,
    }))
}

const RELAY_ERROR_ORIGIN_HEADER: &str = "x-zenith-relay-error-origin";
const RELAY_UPSTREAM_ORIGIN_HEADER: &str = "x-zenith-relay-upstream-origin";

fn fallback_response_origin(response: &Response<Body>) -> ErrorOrigin {
    response
        .headers()
        .get(RELAY_ERROR_ORIGIN_HEADER)
        .or_else(|| response.headers().get(RELAY_UPSTREAM_ORIGIN_HEADER))
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(ErrorOrigin::Relay)
}

fn websocket_transport_fallback_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::BAD_REQUEST
            | StatusCode::NOT_FOUND
            | StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::UPGRADE_REQUIRED
            | StatusCode::NOT_IMPLEMENTED
    )
}

struct Connected {
    upstream: UpstreamWebSocket,
    initial_messages: Vec<UpstreamMessage>,
    route: ExecutorRoute,
    request: ClientRequest,
    lease: CandidateLease,
    attempt: u16,
    started: Instant,
    retry_deadline: Option<TokioInstant>,
}

async fn connect_upstream(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    client_headers: &HeaderMap,
    mut request: ClientRequest,
    allow_previous_response_reset: bool,
    attempt_offset: u16,
    retry_deadline: Option<TokioInstant>,
) -> Result<Connected, GatewayFailure> {
    let mut tried = HashSet::new();
    let mut attempt = attempt_offset;
    let mut attempts_this_run = 0_usize;
    let mut confirmed_response_missing = false;
    let mut native_replay_attempted = false;
    let mut function_item_id_repair_attempted = false;
    let mut custom_tool_item_id_repair_attempted = false;
    let mut message_item_id_repair_attempted = false;
    let mut legacy_call_id_repair_attempted = false;
    let mut model_switch_reset_attempted = false;
    let mut stale_tool_history_recovered = false;
    let mut last_failure: Option<GatewayFailure> = None;
    let mut websocket_http_fallback_origin = None;
    let mut retry_deadline = retry_deadline.or_else(|| {
        (!runtime.chatgpt_retry_until_available())
            .then(|| TokioInstant::now() + Duration::from_millis(runtime.chatgpt_retry_window_ms()))
    });
    let mut retry_wait_attempt = 0u32;
    let mut retry_window_expired = false;
    let mut automatic_recovery = AutomaticRecovery::new();

    'candidates: loop {
        // `ClientRequest` records that this is an eligible managed ChatGPT
        // request. Read the setting on every retry cycle so disabling it
        // releases an active persistent wait after the bounded poll.
        let retry_until_available = runtime.chatgpt_retry_until_available();
        // The request keeps its client eligibility, while the setting is live.
        // If it was enabled after this request began, discard the old bounded
        // deadline before the next retry cycle.
        if retry_until_available {
            retry_deadline = None;
        }
        let wait_for_candidate_availability =
            request.wait_for_candidate_availability && retry_until_available;
        let attempt_limit = runtime
            .max_retry_candidates()
            .saturating_sub(usize::from(attempt_offset));
        if attempts_this_run >= attempt_limit {
            if websocket_http_fallback_origin.is_none()
                && wait_for_candidate_availability
                && last_failure.as_ref().is_none_or(|failure| {
                    super::errors::retryable_failure(
                        failure.status,
                        failure.category,
                        request.has_previous_response_id(),
                    ) && !matches!(
                        failure.category,
                        error_codes::UPSTREAM_UNAUTHORIZED
                            | error_codes::UPSTREAM_ACCOUNT_DISABLED
                            | error_codes::UPSTREAM_USAGE_NOT_INCLUDED
                            | error_codes::UPSTREAM_REGION_UNSUPPORTED
                            | error_codes::UPSTREAM_MODEL_NOT_FOUND
                            | error_codes::UPSTREAM_MODEL_UNSUPPORTED
                            | error_codes::UPSTREAM_FORBIDDEN
                            | error_codes::UPSTREAM_CONTENT_POLICY
                            | error_codes::UPSTREAM_INVALID_REQUEST
                            | error_codes::UPSTREAM_CANDIDATE_REJECTED
                    )
                })
            {
                tried.clear();
                attempts_this_run = 0;
                retry_wait_attempt = retry_wait_attempt.saturating_add(1);
                if !runtime
                    .wait_for_candidate_availability(
                        runtime.earliest_retry_at(
                            key,
                            &request.resolved_model,
                            WEBSOCKET_PROTOCOLS,
                            &tried,
                            request.response_affinity_key.as_deref(),
                            now_ms(),
                        ),
                        websocket_retry_backoff(retry_wait_attempt),
                        retry_deadline,
                    )
                    .await
                {
                    retry_window_expired = true;
                    break;
                }
                continue;
            }
            break;
        }
        let selected = runtime
            .select_and_reserve(
                key,
                &request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                &tried,
                (
                    request.response_affinity_key.as_deref(),
                    request.prompt_affinity_key.as_deref(),
                ),
                now_ms(),
            )
            .await;
        let Some((selected, lease)) = selected else {
            if !request.requires_affinity_owner
                && runtime.release_unroutable_response_affinity(
                    key,
                    &mut request.response_affinity_key,
                    &request.resolved_model,
                    WEBSOCKET_PROTOCOLS,
                    now_ms(),
                )
            {
                continue;
            }
            // A previous response is pinned to its original owner.  When a
            // later WebSocket turn switches to a model that owner cannot
            // structurally serve, affinity selection returns no candidate
            // before an upstream request is attempted.  Clear the opaque
            // continuation once so the new model can use a compatible owner;
            // temporary health, quota, and cooldown misses remain retryable.
            if allow_previous_response_reset
                && request.has_previous_response_id()
                && !request.has_unpaired_tool_output()
                && !model_switch_reset_attempted
                && (request
                    .response_affinity_key
                    .as_deref()
                    .and_then(|affinity_key| {
                        runtime.response_affinity_owner_supports_model(
                            affinity_key,
                            &request.resolved_model,
                            WEBSOCKET_PROTOCOLS,
                            now_ms(),
                        )
                    })
                    == Some(false)
                    || request
                        .response_affinity_key
                        .as_deref()
                        .and_then(|affinity_key| {
                            runtime.response_affinity_owner_supports_route(
                                key,
                                affinity_key,
                                &request.resolved_model,
                                WEBSOCKET_PROTOCOLS,
                                now_ms(),
                            )
                        })
                        == Some(false))
                && request.drop_previous_response_id(runtime, &key.id)
            {
                model_switch_reset_attempted = true;
                continue;
            }
            // The owner may be temporarily ineligible because its quota or
            // cooldown changed after the previous turn. Use the bounded native
            // replay before waiting, then let the next selection choose any
            // compatible candidate (OAuth or API source).
            if request.has_previous_response_id() && request.requires_affinity_owner {
                if let Some(affinity_key) = request.response_affinity_key.clone() {
                    if let Some(owner_candidate_id) =
                        runtime.response_affinity_candidate(&affinity_key, now_ms())
                    {
                        let owner_model = runtime
                            .executor_route(
                                &owner_candidate_id,
                                &request.resolved_model,
                                &key.scope_snapshot(),
                                WEBSOCKET_PROTOCOLS,
                                false,
                            )
                            .map(|route| route.source_model)
                            .unwrap_or_else(|| request.resolved_model.clone());
                        match request.replay_native_continuation(
                            runtime,
                            &key.id,
                            &owner_candidate_id,
                            &owner_model,
                        ) {
                            Ok(true) => {
                                native_replay_attempted = true;
                                tried.clear();
                                continue;
                            }
                            Ok(false) => {}
                            Err(failure) => return Err(failure),
                        }
                    }
                }
            }
            if request.requires_affinity_owner
                && request
                    .response_affinity_key
                    .as_deref()
                    .and_then(|affinity_key| {
                        runtime.response_affinity_owner_supports_route(
                            key,
                            affinity_key,
                            &request.resolved_model,
                            WEBSOCKET_PROTOCOLS,
                            now_ms(),
                        )
                    })
                    == Some(false)
            {
                return Err(GatewayFailure::continuation_unavailable());
            }
            if websocket_http_fallback_origin.is_none()
                && automatic_recovery
                    .retry(
                        &CandidateRetryContext {
                            runtime,
                            key,
                            resolved_model: &request.resolved_model,
                            protocols: WEBSOCKET_PROTOCOLS,
                            exclusions: &HashSet::new(),
                        },
                        &mut tried,
                        request.response_affinity_key.as_deref(),
                    )
                    .await
            {
                continue;
            }
            if websocket_http_fallback_origin.is_none() && wait_for_candidate_availability {
                tried.clear();
                retry_wait_attempt = retry_wait_attempt.saturating_add(1);
                if !runtime
                    .wait_for_candidate_availability(
                        runtime.earliest_retry_at(
                            key,
                            &request.resolved_model,
                            WEBSOCKET_PROTOCOLS,
                            &tried,
                            request.response_affinity_key.as_deref(),
                            now_ms(),
                        ),
                        websocket_retry_backoff(retry_wait_attempt),
                        retry_deadline,
                    )
                    .await
                {
                    retry_window_expired = true;
                    break;
                }
                continue;
            }
            break;
        };
        tried.insert(selected.candidate_id.clone());
        let response_affinity_hit = selected.response_affinity_hit;
        let Some(mut route) = runtime.executor_route(
            &selected.candidate_id,
            &request.resolved_model,
            &key.scope_snapshot(),
            WEBSOCKET_PROTOCOLS,
            false,
        ) else {
            continue;
        };
        request.apply_service_tier_for_route(runtime, &route);
        route.service_tier = request.service_tier(runtime, &route);
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.client_context_id = client_context_fingerprint(client_headers);
        let source_error_origin = route_error_origin(&route);
        if route.wire_api != WireApi::Responses {
            continue;
        }
        if !route.adapter.is_passthrough() {
            runtime.mark_websocket_http_only(
                &route.candidate_id,
                &request.resolved_model,
                now_ms(),
            );
            drop(lease);
            websocket_http_fallback_origin = Some(source_error_origin);
            last_failure = Some(GatewayFailure::websocket_http_fallback(source_error_origin));
            continue;
        }
        if runtime.websocket_is_http_only(&route.candidate_id, &request.resolved_model, now_ms()) {
            drop(lease);
            websocket_http_fallback_origin = Some(source_error_origin);
            last_failure = Some(GatewayFailure::websocket_http_fallback(source_error_origin));
            continue;
        }
        attempt = attempt.saturating_add(1);
        attempts_this_run = attempts_this_run.saturating_add(1);
        let started = Instant::now();
        let prepared = match runtime
            .prepare_authorization(&route.candidate_id, now_ms())
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                let failure = GatewayFailure::prepare(error, source_error_origin);
                record_connect_failure(
                    runtime, key, &route, &request, attempt, started, &failure, None,
                );
                last_failure = Some(failure);
                continue;
            }
        };
        let payload = request.payload_for(&route)?;
        let mut prepared = prepared;
        let mut refresh_fence = None;
        let upgrade = loop {
            let mut headers = upstream_headers(
                client_headers,
                &prepared,
                route.account_id.is_some() && request.responses_lite_for(&route),
                &request.request_id,
            );
            apply_codex_routing_hint(&mut headers, &route.source_model, route.service_tier);
            if let Some(account_id) = route.account_id.as_deref() {
                guard_account_request(runtime, &key.id, &mut headers, account_id, now_ms());
            } else {
                headers.remove(CODEX_TURN_STATE_HEADER);
            }
            let upgrade = runtime
                .websocket_client(&route.candidate_id)
                .get(route.upstream_url.clone())
                .headers(headers)
                .upgrade();
            let Ok(Ok(upgrade)) = timeout(UPSTREAM_CONNECT_TIMEOUT, upgrade.send()).await else {
                let failure = GatewayFailure::transport(source_error_origin);
                record_connect_failure(
                    runtime, key, &route, &request, attempt, started, &failure, None,
                );
                last_failure = Some(failure);
                continue 'candidates;
            };
            if upgrade.status() != StatusCode::UNAUTHORIZED
                || prepared.token_generation.is_none()
                || refresh_fence.is_some()
            {
                break upgrade;
            }
            drop(upgrade);
            refresh_fence = runtime.fence_execution(&route.candidate_id);
            prepared = match runtime
                .refresh_authorization_after_unauthorized(
                    &route.candidate_id,
                    prepared.token_generation,
                    now_ms(),
                )
                .await
            {
                Ok(prepared) => prepared,
                Err(error) => {
                    let failure = GatewayFailure::prepare(error, source_error_origin);
                    record_connect_failure(
                        runtime, key, &route, &request, attempt, started, &failure, None,
                    );
                    last_failure = Some(failure);
                    continue 'candidates;
                }
            };
        };
        drop(refresh_fence);
        // The final prepared authorization may differ from the first attempt
        // after an in-band 401 refresh. Preserve its generation on this
        // request-local route so every later WebSocket usage event is tied to
        // the credential that actually performed the upgrade.
        route.account_token_generation = prepared.token_generation;
        let status = upgrade.status();
        let response_headers = upgrade.headers().clone();
        runtime.observe_codex_quota_headers(
            &route.candidate_id,
            status,
            &response_headers,
            now_ms(),
        );
        if status == StatusCode::SWITCHING_PROTOCOLS {
            if let Some(account_id) = route.account_id.as_deref() {
                note_account_response_header(
                    runtime,
                    &key.id,
                    client_headers,
                    account_id,
                    &response_headers,
                    now_ms(),
                );
            }
        }
        if status != StatusCode::SWITCHING_PROTOCOLS {
            let response = upgrade.into_inner();
            let body = timeout(
                UPSTREAM_CONNECT_TIMEOUT,
                crate::transport::collect_limited(response, MAX_WEBSOCKET_ERROR_BYTES),
            )
            .await
            .ok()
            .and_then(Result::ok);
            let failure =
                GatewayFailure::upstream_status(status, body.as_deref(), source_error_origin);
            if !legacy_call_id_repair_attempted
                && body
                    .as_deref()
                    .is_some_and(super::errors::responses_call_id_is_missing)
                && request.repair_legacy_call_ids()
            {
                legacy_call_id_repair_attempted = true;
                attempt = attempt.saturating_sub(1);
                attempts_this_run = attempts_this_run.saturating_sub(1);
                tried.remove(&route.candidate_id);
                continue 'candidates;
            }
            if websocket_transport_fallback_status(status) {
                runtime.mark_websocket_http_only(
                    &route.candidate_id,
                    &request.resolved_model,
                    now_ms(),
                );
                websocket_http_fallback_origin = Some(source_error_origin);
                last_failure = Some(GatewayFailure::websocket_http_fallback(source_error_origin));
                continue 'candidates;
            }
            let response_missing = body
                .as_deref()
                .is_some_and(super::errors::previous_response_not_found);
            let affinity_miss = super::errors::recoverable_response_affinity_miss(
                status,
                request.has_previous_response_id(),
                response_affinity_hit,
                response_missing,
            );
            let model_switch_reset = !model_switch_reset_attempted
                && super::errors::recoverable_response_model_switch(
                    status,
                    failure.category,
                    request.has_previous_response_id(),
                    request.has_unpaired_tool_output(),
                    body.as_deref().unwrap_or_default(),
                );
            if affinity_miss
                && response_affinity_hit
                && request.replay_missing_response(
                    runtime,
                    &key.id,
                    &route,
                    &mut native_replay_attempted,
                )?
            {
                record_connect_affinity_miss(
                    runtime, key, &route, &request, attempt, started, status,
                );
                tried.remove(&route.candidate_id);
                last_failure = Some(failure);
                continue 'candidates;
            }
            let stale_tool_history = !stale_tool_history_recovered
                && request.has_previous_response_id()
                && body
                    .as_deref()
                    .is_some_and(super::errors::responses_tool_call_is_missing_output)
                && request.drop_previous_response_id(runtime, &key.id);
            if stale_tool_history {
                stale_tool_history_recovered = true;
                record_connect_rejection(
                    runtime, key, &route, &request, attempt, started, &failure,
                );
                last_failure = Some(failure);
                continue;
            }
            if model_switch_reset && request.drop_previous_response_id(runtime, &key.id) {
                model_switch_reset_attempted = true;
                last_failure = Some(failure);
                continue;
            }
            if body
                .as_deref()
                .is_some_and(super::errors::prompt_cache_write_rejected)
            {
                runtime.invalidate_prompt_affinity(request.prompt_affinity_key.as_deref());
                record_connect_failure_with_hint(
                    runtime,
                    key,
                    &route,
                    &request,
                    attempt,
                    started,
                    &failure,
                    Some(&response_headers),
                    body.as_deref()
                        .map(rate_limit_body_hint)
                        .unwrap_or_default(),
                );
                last_failure = Some(failure);
                continue;
            }
            if affinity_miss {
                confirmed_response_missing |= response_missing;
                runtime.invalidate_response_affinity(request.response_affinity_key.as_deref());
                record_connect_affinity_miss(
                    runtime, key, &route, &request, attempt, started, status,
                );
                last_failure = Some(failure);
                if response_missing && response_affinity_hit {
                    break;
                }
                continue;
            }
            if super::errors::retryable_failure(
                status,
                failure.category,
                request.has_previous_response_id(),
            ) {
                if response_affinity_hit && !request.requires_affinity_owner {
                    request.response_affinity_key = None;
                }
                record_connect_failure_with_hint(
                    runtime,
                    key,
                    &route,
                    &request,
                    attempt,
                    started,
                    &failure,
                    Some(&response_headers),
                    body.as_deref()
                        .map(rate_limit_body_hint)
                        .unwrap_or_default(),
                );
                last_failure = Some(failure);
                continue;
            }
            record_connect_rejection(runtime, key, &route, &request, attempt, started, &failure);
            return Err(failure);
        }
        let Ok(Ok(mut upstream)) =
            timeout(UPSTREAM_CONNECT_TIMEOUT, upgrade.into_websocket()).await
        else {
            let failure = GatewayFailure::transport(source_error_origin);
            record_connect_failure(
                runtime, key, &route, &request, attempt, started, &failure, None,
            );
            last_failure = Some(failure);
            continue;
        };
        runtime.mark_websocket_supported(&route.candidate_id, &request.resolved_model);
        if send_request(&mut upstream, payload, source_error_origin)
            .await
            .is_err()
        {
            let failure = GatewayFailure::transport(source_error_origin);
            record_connect_failure(
                runtime, key, &route, &request, attempt, started, &failure, None,
            );
            last_failure = Some(failure);
            continue;
        }
        let initial_messages =
            match initial_application_messages(&mut upstream, source_error_origin).await {
                Ok(messages) => messages,
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
        if initial_messages_are_empty_incomplete(&initial_messages) {
            let failure = GatewayFailure::classified(
                StatusCode::BAD_GATEWAY,
                error_codes::STREAM_INCOMPLETE,
                source_error_origin,
            );
            record_connect_failure(
                runtime, key, &route, &request, attempt, started, &failure, None,
            );
            last_failure = Some(failure);
            continue;
        }
        if let Some(terminal) = initial_messages.last().and_then(first_message_terminal) {
            if terminal.outcome == Some(EventTerminalOutcome::Failure) {
                let terminal_body = initial_messages.last().and_then(|message| match message {
                    UpstreamMessage::Text(text) => Some(text.as_bytes()),
                    UpstreamMessage::Binary(bytes) => Some(bytes.as_ref()),
                    _ => None,
                });
                if !function_item_id_repair_attempted
                    && terminal_body
                        .is_some_and(super::errors::responses_function_item_id_requires_fc_prefix)
                    && request.repair_function_item_ids()
                {
                    function_item_id_repair_attempted = true;
                    tried.remove(&route.candidate_id);
                    last_failure = None;
                    continue;
                }
                if !custom_tool_item_id_repair_attempted
                    && terminal_body.is_some_and(
                        super::errors::responses_custom_tool_item_id_requires_ctc_prefix,
                    )
                    && request.repair_custom_tool_item_ids()
                {
                    custom_tool_item_id_repair_attempted = true;
                    tried.remove(&route.candidate_id);
                    last_failure = None;
                    continue;
                }
                if !message_item_id_repair_attempted
                    && terminal_body
                        .is_some_and(super::errors::responses_message_item_id_requires_msg_prefix)
                    && request.repair_message_item_ids()
                {
                    message_item_id_repair_attempted = true;
                    tried.remove(&route.candidate_id);
                    last_failure = None;
                    continue;
                }
                let category = terminal.error_category.unwrap_or_else(|| {
                    super::errors::classify_upstream_error(
                        terminal_failure_status(terminal.status),
                        None,
                    )
                    .category
                });
                let status = terminal
                    .status
                    .filter(|status| !status.is_success())
                    .unwrap_or_else(|| super::errors::upstream_failure_status(category));
                if !legacy_call_id_repair_attempted
                    && terminal_body.is_some_and(super::errors::responses_call_id_is_missing)
                    && request.repair_legacy_call_ids()
                {
                    legacy_call_id_repair_attempted = true;
                    attempt = attempt.saturating_sub(1);
                    attempts_this_run = attempts_this_run.saturating_sub(1);
                    tried.remove(&route.candidate_id);
                    continue 'candidates;
                }
                let affinity_miss = super::errors::recoverable_response_affinity_miss(
                    status,
                    request.has_previous_response_id(),
                    response_affinity_hit,
                    terminal.previous_response_not_found,
                );
                let failure = GatewayFailure::classified(status, category, source_error_origin)
                    .with_upstream_error(terminal.upstream_error.clone());
                if affinity_miss
                    && response_affinity_hit
                    && request.replay_missing_response(
                        runtime,
                        &key.id,
                        &route,
                        &mut native_replay_attempted,
                    )?
                {
                    record_connect_affinity_miss(
                        runtime, key, &route, &request, attempt, started, status,
                    );
                    tried.remove(&route.candidate_id);
                    last_failure = Some(failure);
                    continue 'candidates;
                }
                if !stale_tool_history_recovered
                    && request.has_previous_response_id()
                    && terminal_body
                        .is_some_and(super::errors::responses_tool_call_is_missing_output)
                    && request.drop_previous_response_id(runtime, &key.id)
                {
                    stale_tool_history_recovered = true;
                    let failure = GatewayFailure::classified(status, category, source_error_origin)
                        .with_upstream_error(terminal.upstream_error.clone());
                    record_connect_rejection(
                        runtime, key, &route, &request, attempt, started, &failure,
                    );
                    last_failure = Some(failure);
                    continue;
                }
                if !model_switch_reset_attempted
                    && super::errors::recoverable_response_model_switch(
                        status,
                        category,
                        request.has_previous_response_id(),
                        request.has_unpaired_tool_output(),
                        terminal_body.unwrap_or_default(),
                    )
                    && request.drop_previous_response_id(runtime, &key.id)
                {
                    model_switch_reset_attempted = true;
                    let failure = GatewayFailure::classified(status, category, source_error_origin)
                        .with_upstream_error(terminal.upstream_error.clone());
                    record_connect_rejection(
                        runtime, key, &route, &request, attempt, started, &failure,
                    );
                    last_failure = Some(failure);
                    continue;
                }
                if terminal_body.is_some_and(super::errors::prompt_cache_write_rejected) {
                    runtime.invalidate_prompt_affinity(request.prompt_affinity_key.as_deref());
                    let failure = GatewayFailure::classified(status, category, source_error_origin)
                        .with_upstream_error(terminal.upstream_error.clone());
                    record_connect_failure_with_hint(
                        runtime,
                        key,
                        &route,
                        &request,
                        attempt,
                        started,
                        &failure,
                        Some(&terminal.headers),
                        terminal.body_hint,
                    );
                    last_failure = Some(failure);
                    continue;
                }
                if affinity_miss
                    || super::errors::retryable_failure(
                        status,
                        category,
                        request.has_previous_response_id(),
                    )
                {
                    let failure = GatewayFailure::classified(status, category, source_error_origin)
                        .with_upstream_error(terminal.upstream_error.clone());
                    if affinity_miss {
                        confirmed_response_missing |= terminal.previous_response_not_found;
                        runtime
                            .invalidate_response_affinity(request.response_affinity_key.as_deref());
                        record_connect_affinity_miss(
                            runtime, key, &route, &request, attempt, started, status,
                        );
                    } else {
                        record_connect_failure_with_hint(
                            runtime,
                            key,
                            &route,
                            &request,
                            attempt,
                            started,
                            &failure,
                            Some(&terminal.headers),
                            terminal.body_hint,
                        );
                        if response_affinity_hit && !request.requires_affinity_owner {
                            request.response_affinity_key = None;
                        }
                    }
                    last_failure = Some(failure);
                    if affinity_miss
                        && terminal.previous_response_not_found
                        && response_affinity_hit
                    {
                        break;
                    }
                    continue;
                }
            }
        }
        return Ok(Connected {
            upstream,
            initial_messages,
            route,
            request,
            lease,
            attempt,
            started,
            retry_deadline,
        });
    }

    if allow_previous_response_reset
        && request.has_previous_response_id()
        && confirmed_response_missing
    {
        let mut reset_request = request.clone();
        if reset_request.drop_previous_response_id(runtime, &key.id) {
            return Box::pin(connect_upstream(
                runtime,
                key,
                client_headers,
                reset_request,
                false,
                attempt,
                retry_deadline,
            ))
            .await;
        }
        return Err(GatewayFailure::continuation_unavailable());
    }

    if retry_window_expired {
        return Err(GatewayFailure::classified(
            StatusCode::SERVICE_UNAVAILABLE,
            error_codes::UPSTREAM_UNAVAILABLE,
            ErrorOrigin::Relay,
        ));
    }
    if let Some(origin) = websocket_http_fallback_origin {
        return Err(GatewayFailure::websocket_http_fallback(origin));
    }
    if let Some(retry_at_ms) = runtime.earliest_retry_at(
        key,
        &request.resolved_model,
        WEBSOCKET_PROTOCOLS,
        &HashSet::new(),
        request.response_affinity_key.as_deref(),
        now_ms(),
    ) {
        return Err(GatewayFailure::cooldown(retry_at_ms));
    }
    Err(last_failure.unwrap_or_else(GatewayFailure::unavailable))
}

/// Keeps a persistent pre-output retry cancellable by the client. Candidate
/// recovery may wait indefinitely, but a closed client WebSocket must release
/// the request and its lease immediately.
#[allow(clippy::too_many_arguments)]
async fn connect_upstream_while_client_connected(
    downstream: &mut WebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    client_headers: &HeaderMap,
    request: ClientRequest,
    allow_previous_response_reset: bool,
    attempt_offset: u16,
    retry_deadline: Option<TokioInstant>,
) -> Result<Connected, GatewayFailure> {
    await_while_client_connected(
        downstream,
        connect_upstream(
            runtime,
            key,
            client_headers,
            request,
            allow_previous_response_reset,
            attempt_offset,
            retry_deadline,
        ),
    )
    .await?
}

async fn await_while_client_connected<F: std::future::Future>(
    downstream: &mut WebSocket,
    future: F,
) -> Result<F::Output, GatewayFailure> {
    tokio::pin!(future);
    loop {
        tokio::select! {
            result = &mut future => return Ok(result),
            message = downstream.recv() => {
                match message {
                    Some(Ok(Message::Ping(payload))) => {
                        downstream
                            .send(Message::Pong(payload))
                            .await
                            .map_err(|_| GatewayFailure::client_closed())?;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                        return Err(GatewayFailure::client_closed());
                    }
                    Some(Ok(Message::Text(_))) | Some(Ok(Message::Binary(_))) => {
                        return Err(GatewayFailure::invalid_request(
                            "a response is already in progress",
                        ));
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn retry_upstream_connection(
    downstream: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    headers: &HeaderMap,
    state: &mut BridgeState,
    request: ClientRequest,
    attempt_offset: u16,
    retry_deadline: Option<TokioInstant>,
    request_id: Option<&str>,
) -> bool {
    match connect_upstream_while_client_connected(
        downstream,
        runtime,
        key,
        headers,
        request,
        true,
        attempt_offset,
        retry_deadline,
    )
    .await
    {
        Ok(connected) => {
            install_connected(downstream, upstream, runtime, key, state, connected).await
        }
        Err(retry_failure) => {
            send_gateway_error(downstream, &retry_failure, request_id).await;
            false
        }
    }
}

fn websocket_retry_backoff(attempt: u32) -> Duration {
    let exponent = attempt.min(6);
    let base_ms = 100u64.saturating_mul(1u64 << exponent);
    let jitter_ms = u64::from((attempt.wrapping_mul(37)) % 100);
    Duration::from_millis((base_ms + jitter_ms).min(5_000))
}

async fn initial_application_messages(
    upstream: &mut UpstreamWebSocket,
    origin: ErrorOrigin,
) -> Result<Vec<UpstreamMessage>, GatewayFailure> {
    let mut messages = Vec::new();
    let mut buffered_bytes = 0_usize;
    loop {
        let message = first_application_message(upstream, origin).await?;
        let message_bytes = match &message {
            UpstreamMessage::Text(text) => text.len(),
            UpstreamMessage::Binary(bytes) => bytes.len(),
            _ => 0,
        };
        buffered_bytes = buffered_bytes.saturating_add(message_bytes);
        if buffered_bytes > MAX_WEBSOCKET_MESSAGE_BYTES.saturating_mul(2) {
            return Err(GatewayFailure::message_too_large(origin));
        }
        let (has_output, terminal) = initial_message_state(&message);
        let committed = has_output || terminal.outcome.is_some();
        messages.push(message);
        if committed {
            return Ok(messages);
        }
    }
}

async fn first_application_message(
    upstream: &mut UpstreamWebSocket,
    origin: ErrorOrigin,
) -> Result<UpstreamMessage, GatewayFailure> {
    let deadline = TokioInstant::now() + INITIAL_MESSAGE_TIMEOUT;
    let mut heartbeat = interval_at(
        TokioInstant::now() + WEBSOCKET_HEARTBEAT_INTERVAL,
        WEBSOCKET_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = sleep_until(deadline) => return Err(GatewayFailure::idle_timeout(origin)),
            _ = heartbeat.tick() => {
                upstream
                    .send(UpstreamMessage::Ping(Default::default()))
                    .await
                    .map_err(|_| GatewayFailure::transport(origin))?;
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
                            .map_err(|_| GatewayFailure::transport(origin))?;
                    }
                    Some(Ok(UpstreamMessage::Pong(_))) => {}
                    Some(Ok(UpstreamMessage::Close { .. })) | None => {
                        return Err(GatewayFailure::closed(origin));
                    }
                    Some(Err(_)) => return Err(GatewayFailure::transport(origin)),
                }
            }
        }
    }
}

fn first_message_terminal(message: &UpstreamMessage) -> Option<EventTerminal> {
    Some(initial_message_state(message).1)
}

fn initial_message_state(message: &UpstreamMessage) -> (bool, EventTerminal) {
    let payload = match message {
        UpstreamMessage::Text(text) => text.as_bytes(),
        UpstreamMessage::Binary(bytes) => bytes.as_ref(),
        // This function is only called for application messages, but retain
        // conservative behavior if that invariant changes.
        _ => return (true, EventTerminal::default()),
    };
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        // A malformed frame has already reached the bridge and must never be
        // replayed to another account as if it were setup metadata.
        return (true, EventTerminal::default());
    };
    let event_type = value.get("type").and_then(Value::as_str);
    (
        has_semantic_output(&value, event_type),
        event_terminal(&value),
    )
}

fn initial_messages_are_empty_incomplete(messages: &[UpstreamMessage]) -> bool {
    let payloads = messages
        .iter()
        .filter_map(|message| match message {
            UpstreamMessage::Text(text) => serde_json::from_slice(text.as_bytes()).ok(),
            UpstreamMessage::Binary(bytes) => serde_json::from_slice(bytes.as_ref()).ok(),
            _ => None,
        })
        .collect::<Vec<Value>>();
    initial_payloads_are_empty_incomplete(&payloads)
}

fn initial_payloads_are_empty_incomplete(payloads: &[Value]) -> bool {
    let Some(terminal) = payloads.last() else {
        return false;
    };
    if terminal.get("type").and_then(Value::as_str) != Some("response.incomplete") {
        return false;
    }
    let saw_output = payloads
        .iter()
        .any(|payload| has_semantic_output(payload, payload.get("type").and_then(Value::as_str)));
    let completed_output_items = payloads
        .iter()
        .filter(|payload| {
            let event_type = payload.get("type").and_then(Value::as_str);
            event_type == Some("response.output_item.done")
                && !is_compaction_payload(payload, event_type)
        })
        .count();
    is_empty_responses_incomplete(terminal, saw_output, completed_output_items)
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
    record_connect_failure_with_hint(
        runtime,
        key,
        route,
        request,
        attempt,
        started,
        failure,
        headers,
        RateLimitBodyHint::default(),
    );
}

#[allow(clippy::too_many_arguments)]
fn record_connect_failure_with_hint(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    route: &ExecutorRoute,
    request: &ClientRequest,
    attempt: u16,
    started: Instant,
    failure: &GatewayFailure,
    headers: Option<&HeaderMap>,
    hint: RateLimitBodyHint,
) {
    let cooldown_context = CooldownContext {
        scope: &route.scope,
        allowed_protocols: &route.allowed_protocols,
    };
    let state = match headers {
        Some(headers) => apply_failure_cooldown_with_hint(
            runtime,
            &route.candidate_id,
            &route.source_model,
            failure.status,
            failure.category,
            headers,
            hint,
            &cooldown_context,
            route.half_open_probe,
        ),
        None => apply_failure_cooldown_with_hint(
            runtime,
            &route.candidate_id,
            &route.source_model,
            failure.status,
            failure.category,
            &HeaderMap::new(),
            hint,
            &cooldown_context,
            route.half_open_probe,
        ),
    };
    let mut event = usage_event(
        &request.request_id,
        attempt,
        &key.id,
        route,
        Some(&request.reasoning_effort_for(route)),
        &request.requested_model,
        false,
        failure.status.as_u16(),
        Some(failure.category.to_string()),
        started.elapsed().as_millis() as u64,
        request.tool_use_for(route),
    );
    apply_failure_state(&mut event, state);
    event.upstream_error = failure.upstream_error.as_deref().cloned();
    emit_usage(runtime, event);
}

fn record_connect_affinity_miss(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    route: &ExecutorRoute,
    request: &ClientRequest,
    attempt: u16,
    started: Instant,
    status: StatusCode,
) {
    emit_usage(
        runtime,
        usage_event(
            &request.request_id,
            attempt,
            &key.id,
            route,
            Some(&request.reasoning_effort_for(route)),
            &request.requested_model,
            false,
            status.as_u16(),
            Some(error_codes::RESPONSE_AFFINITY_MISS.to_string()),
            started.elapsed().as_millis() as u64,
            request.tool_use_for(route),
        ),
    );
}

fn record_connect_rejection(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    route: &ExecutorRoute,
    request: &ClientRequest,
    attempt: u16,
    started: Instant,
    failure: &GatewayFailure,
) {
    let mut event = usage_event(
        &request.request_id,
        attempt,
        &key.id,
        route,
        Some(&request.reasoning_effort_for(route)),
        &request.requested_model,
        false,
        failure.status.as_u16(),
        Some(failure.category.to_string()),
        started.elapsed().as_millis() as u64,
        request.tool_use_for(route),
    );
    event.upstream_error = failure.upstream_error.as_deref().cloned();
    emit_usage(runtime, event);
}

fn upstream_headers(
    client_headers: &HeaderMap,
    prepared: &crate::runtime::PreparedAuthorization,
    responses_lite: bool,
    request_id: &str,
) -> HeaderMap {
    let mut headers = forwarded_codex_headers(client_headers, request_id);
    headers.insert(AUTHORIZATION, prepared.authorization.clone());
    if let Some(identity) = prepared.identity.as_ref() {
        let identity = codex_client_version(client_headers)
            .and_then(|version| identity.with_client_version(version).ok())
            .unwrap_or_else(|| identity.clone());
        identity.insert(&mut headers);
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
    origin: ErrorOrigin,
) -> Result<(), GatewayFailure> {
    let send = async {
        upstream.send(UpstreamMessage::Text(payload)).await?;
        upstream.flush().await
    };
    match timeout(UPSTREAM_CONNECT_TIMEOUT, send).await {
        Ok(Ok(())) => Ok(()),
        _ => Err(GatewayFailure::transport(origin)),
    }
}

struct InFlight {
    request: ClientRequest,
    route: ExecutorRoute,
    event: UsageEvent,
    started: Instant,
    response_id: Option<String>,
    prompt_affinity_key: Option<String>,
    retry_deadline: Option<TokioInstant>,
    client_visible_output: bool,
    legacy_call_id_repair_attempted: bool,
    native_replay_capture: NativeReplayCapture,
}

struct BridgeState {
    local_key_id: String,
    lease: Option<CandidateLease>,
    in_flight: Option<InFlight>,
    stream_id: Option<String>,
    upstream_candidate_id: String,
    upstream_origin: ErrorOrigin,
    last_response_id: Option<String>,
    transient_response_affinity_key: Option<String>,
}

impl BridgeState {
    fn can_send_gateway_error(&self) -> bool {
        self.in_flight
            .as_ref()
            .is_none_or(|in_flight| !in_flight.client_visible_output)
    }

    fn request_id(&self) -> Option<&str> {
        self.in_flight
            .as_ref()
            .map(|in_flight| in_flight.event.request_id.as_str())
    }
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
        &connected.request.request_id,
        connected.attempt,
        &key.id,
        &connected.route,
        Some(&connected.request.reasoning_effort_for(&connected.route)),
        &connected.request.requested_model,
        true,
        StatusCode::OK.as_u16(),
        None,
        0,
        connected.request.tool_use_for(&connected.route),
    );
    let upstream_candidate_id = connected.route.candidate_id.clone();
    let upstream_origin = route_error_origin(&connected.route);
    let prompt_affinity_key = connected.request.prompt_affinity_key.clone();
    let mut state = BridgeState {
        local_key_id: key.id.clone(),
        lease: Some(connected.lease),
        in_flight: Some(InFlight {
            request: connected.request.clone(),
            route: connected.route,
            event: initial_event,
            started: connected.started,
            response_id: None,
            prompt_affinity_key,
            retry_deadline: connected.retry_deadline,
            client_visible_output: false,
            legacy_call_id_repair_attempted: false,
            native_replay_capture: NativeReplayCapture::default(),
        }),
        stream_id: connected.request.stream_id.clone(),
        upstream_candidate_id,
        upstream_origin,
        last_response_id: None,
        transient_response_affinity_key: None,
    };
    for message in connected.initial_messages {
        if !handle_upstream_message(&mut downstream, &runtime, &mut state, message).await {
            return;
        }
    }
    let mut last_activity = TokioInstant::now();
    let mut heartbeat = interval_at(
        TokioInstant::now() + WEBSOCKET_HEARTBEAT_INTERVAL,
        WEBSOCKET_HEARTBEAT_INTERVAL,
    );
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        let idle_deadline = last_activity + WEBSOCKET_IDLE_TIMEOUT;
        let semantic_waiting = state
            .in_flight
            .as_ref()
            .is_some_and(|in_flight| in_flight.event.ttft_ms.is_none());
        let semantic_deadline = TokioInstant::now()
            + state
                .in_flight
                .as_ref()
                .map_or(WEBSOCKET_SEMANTIC_TIMEOUT, |in_flight| {
                    WEBSOCKET_SEMANTIC_TIMEOUT.saturating_sub(in_flight.started.elapsed())
                });
        tokio::select! {
            _ = sleep_until(semantic_deadline), if semantic_waiting => {
                let request_id = state.request_id().map(str::to_owned);
                let can_send_error = state.can_send_gateway_error();
                finish_incomplete(&runtime, &mut state, error_codes::STREAM_SEMANTIC_TIMEOUT);
                if can_send_error {
                send_gateway_error(
                    &mut downstream,
                    &GatewayFailure::semantic_timeout(state.upstream_origin),
                    request_id.as_deref(),
                ).await;
                }
                break;
            }
            _ = sleep_until(idle_deadline) => {
                let active_request = state.in_flight.is_some() && state.can_send_gateway_error();
                let request_id = state.request_id().map(str::to_owned);
                finish_incomplete(&runtime, &mut state, error_codes::WEBSOCKET_IDLE_TIMEOUT);
                if active_request {
                    send_gateway_error(
                        &mut downstream,
                        &GatewayFailure::idle_timeout(state.upstream_origin),
                        request_id.as_deref(),
                    ).await;
                } else {
                    let _ = downstream.send(Message::Close(Some(CloseFrame {
                        code: close_code::AWAY,
                        reason: "idle timeout".into(),
                    }))).await;
                }
                break;
            }
            _ = heartbeat.tick() => {
                if upstream.send(UpstreamMessage::Ping(Default::default())).await.is_err() {
                    let active_request = state.in_flight.is_some() && state.can_send_gateway_error();
                    let request_id = state.request_id().map(str::to_owned);
                    finish_incomplete(&runtime, &mut state, error_codes::UPSTREAM_WEBSOCKET);
                    if active_request {
                        send_gateway_error(
                            &mut downstream,
                            &GatewayFailure::transport(state.upstream_origin),
                            request_id.as_deref(),
                        ).await;
                    }
                    break;
                }
            }
            message = downstream.recv() => {
                last_activity = TokioInstant::now();
                let Some(message) = message else {
                    finish_incomplete(&runtime, &mut state, error_codes::CLIENT_CANCELLED);
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
                        let request_id = state.request_id().map(str::to_owned);
                        let can_send_error = state.can_send_gateway_error();
                        finish_incomplete(&runtime, &mut state, failure.category);
                        if can_send_error {
                        send_gateway_error(&mut downstream, &failure, request_id.as_deref()).await;
                        }
                        break;
                    }
                }
            }
            message = upstream.next() => {
                last_activity = TokioInstant::now();
                let (message, failure) = match message {
                    Some(Ok(message @ (UpstreamMessage::Text(_)
                        | UpstreamMessage::Binary(_)
                        | UpstreamMessage::Ping(_)
                        | UpstreamMessage::Pong(_)))) => (Some(message), None),
                    Some(Ok(UpstreamMessage::Close { .. })) | None => {
                        (None, Some(GatewayFailure::closed(state.upstream_origin)))
                    }
                    Some(Err(_)) => {
                        (None, Some(GatewayFailure::transport(state.upstream_origin)))
                    }
                };
                let Some(message) = message else {
                    let category = failure
                        .as_ref()
                        .map(|failure| failure.category)
                        .unwrap_or(error_codes::UPSTREAM_WEBSOCKET_CLOSED);
                    let request_id = state.request_id().map(str::to_owned);
                    if let Some(request) = retryable_disconnect_request(&runtime, &state) {
                        let attempt_offset = state
                            .in_flight
                            .as_ref()
                            .map(|in_flight| in_flight.event.attempt)
                            .unwrap_or_default();
                        let retry_deadline = state
                            .in_flight
                            .as_ref()
                            .and_then(|in_flight| in_flight.retry_deadline);
                        finish_incomplete(&runtime, &mut state, category);
                        if retry_upstream_connection(
                            &mut downstream,
                            &mut upstream,
                            &runtime,
                            &key,
                            &headers,
                            &mut state,
                            request,
                            attempt_offset,
                            retry_deadline,
                            request_id.as_deref(),
                        )
                        .await
                        {
                            continue;
                        }
                        break;
                    }
                    let active_request = state.in_flight.is_some();
                    let can_send_error = state.can_send_gateway_error();
                    finish_incomplete(&runtime, &mut state, category);
                    if active_request && can_send_error {
                        if let Some(failure) = failure {
                            send_gateway_error(
                                &mut downstream,
                                &failure,
                                request_id.as_deref(),
                            )
                            .await;
                        }
                    } else if active_request {
                        let _ = downstream
                            .send(Message::Close(Some(CloseFrame {
                                code: close_code::ERROR,
                                reason: "upstream stream ended after output".into(),
                            })))
                            .await;
                    }
                    break;
                };
                if let Some(request) = repairable_terminal_request(&runtime, &mut state, &message) {
                    let request_id = state.request_id().map(str::to_owned);
                    let attempt_offset = state
                        .in_flight
                        .as_ref()
                        .map(|in_flight| in_flight.event.attempt)
                        .unwrap_or_default();
                    let retry_deadline = state
                        .in_flight
                        .as_ref()
                        .and_then(|in_flight| in_flight.retry_deadline);
                    let mut terminal = match &message {
                        UpstreamMessage::Text(text) => {
                            inspect_upstream_event(text.as_bytes(), &mut state)
                        }
                        UpstreamMessage::Binary(bytes) => inspect_upstream_event(bytes, &mut state),
                        _ => EventTerminal::default(),
                    };
                    if terminal.previous_response_not_found {
                        terminal.error_category = Some(error_codes::RESPONSE_AFFINITY_MISS);
                    }
                    finish_terminal(&runtime, &mut state, terminal);
                    if retry_upstream_connection(
                        &mut downstream,
                        &mut upstream,
                        &runtime,
                        &key,
                        &headers,
                        &mut state,
                        request,
                        attempt_offset,
                        retry_deadline,
                        request_id.as_deref(),
                    )
                    .await
                    {
                        continue;
                    }
                    break;
                }
                if let Some(request) = retryable_terminal_request(&runtime, &state, &message) {
                    let request_id = state.request_id().map(str::to_owned);
                    let attempt_offset = state
                        .in_flight
                        .as_ref()
                        .map(|in_flight| in_flight.event.attempt)
                        .unwrap_or_default();
                    let retry_deadline = state
                        .in_flight
                        .as_ref()
                        .and_then(|in_flight| in_flight.retry_deadline);
                    let terminal = match &message {
                        UpstreamMessage::Text(text) => inspect_upstream_event(text.as_bytes(), &mut state),
                        UpstreamMessage::Binary(bytes) => inspect_upstream_event(bytes, &mut state),
                        _ => EventTerminal::default(),
                    };
                    // Do not expose a retryable pre-output terminal failure to
                    // the client. `finish_terminal` retains its usual health,
                    // quota, telemetry, and lease-settlement behavior first.
                    finish_terminal(&runtime, &mut state, terminal);
                    if retry_upstream_connection(
                        &mut downstream,
                        &mut upstream,
                        &runtime,
                        &key,
                        &headers,
                        &mut state,
                        request,
                        attempt_offset,
                        retry_deadline,
                        request_id.as_deref(),
                    )
                    .await
                    {
                        continue;
                    }
                    break;
                }
                if !handle_upstream_message(&mut downstream, &runtime, &mut state, message).await {
                    break;
                }
            }
        }
    }
}

fn retryable_disconnect_request(
    runtime: &GatewayRuntime,
    state: &BridgeState,
) -> Option<ClientRequest> {
    let in_flight = state.in_flight.as_ref()?;
    if in_flight.client_visible_output
        || in_flight.event.ttft_ms.is_some()
        || in_flight.event.output_tokens.is_some()
        || in_flight.event.tool_use.tool_call_count > 0
        || in_flight.event.tool_use.text_output
    {
        return None;
    }
    if in_flight.request.has_previous_response_id() {
        return replay_in_flight_continuation(runtime, state);
    }
    if in_flight.request.has_unpaired_tool_output() {
        return None;
    }
    Some(in_flight.request.clone())
}

fn replay_in_flight_continuation(
    runtime: &GatewayRuntime,
    state: &BridgeState,
) -> Option<ClientRequest> {
    let in_flight = state.in_flight.as_ref()?;
    let mut request = in_flight.request.clone();
    request
        .replay_native_continuation(
            runtime,
            &state.local_key_id,
            &in_flight.route.candidate_id,
            &in_flight.route.source_model,
        )
        .ok()?
        .then_some(request)
}

fn retryable_terminal_request(
    runtime: &GatewayRuntime,
    state: &BridgeState,
    message: &UpstreamMessage,
) -> Option<ClientRequest> {
    let terminal = first_message_terminal(message)?;
    if terminal.outcome != Some(EventTerminalOutcome::Failure) {
        return None;
    }
    let in_flight = state.in_flight.as_ref()?;
    if in_flight.client_visible_output
        || in_flight.event.ttft_ms.is_some()
        || in_flight.event.output_tokens.is_some()
        || in_flight.event.tool_use.tool_call_count > 0
        || in_flight.event.tool_use.text_output
    {
        return None;
    }
    let category = terminal.error_category.unwrap_or_else(|| {
        super::errors::classify_upstream_error(terminal_failure_status(terminal.status), None)
            .category
    });
    let status = terminal
        .status
        .filter(|status| !status.is_success())
        .unwrap_or_else(|| super::errors::upstream_failure_status(category));
    let status = super::errors::canonical_upstream_status(status, category);
    if in_flight.request.has_previous_response_id() {
        return super::errors::retryable_failure(status, category, true)
            .then(|| replay_in_flight_continuation(runtime, state))
            .flatten();
    }
    let persistent = runtime.chatgpt_retry_until_available()
        && in_flight.request.wait_for_candidate_availability;
    if (!runtime.automatic_recovery_enabled() && !persistent)
        || in_flight.request.has_unpaired_tool_output()
    {
        return None;
    }
    if !super::errors::retryable_failure(status, category, false)
        || (!runtime.automatic_recovery_enabled()
            && matches!(
                category,
                error_codes::UPSTREAM_UNAUTHORIZED
                    | error_codes::UPSTREAM_ACCOUNT_DISABLED
                    | error_codes::UPSTREAM_USAGE_NOT_INCLUDED
                    | error_codes::UPSTREAM_REGION_UNSUPPORTED
                    | error_codes::UPSTREAM_MODEL_NOT_FOUND
                    | error_codes::UPSTREAM_MODEL_UNSUPPORTED
                    | error_codes::UPSTREAM_FORBIDDEN
                    | error_codes::UPSTREAM_CONTENT_POLICY
                    | error_codes::UPSTREAM_INVALID_REQUEST
                    | error_codes::UPSTREAM_CANDIDATE_REJECTED
            ))
    {
        return None;
    }
    Some(in_flight.request.clone())
}

fn repairable_terminal_request(
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    message: &UpstreamMessage,
) -> Option<ClientRequest> {
    let terminal = first_message_terminal(message)?;
    if terminal.outcome != Some(EventTerminalOutcome::Failure) {
        return None;
    }
    if terminal.previous_response_not_found {
        let in_flight = state.in_flight.as_ref()?;
        if in_flight.client_visible_output {
            return None;
        }
        let request = replay_in_flight_continuation(runtime, state)?;
        return Some(request);
    }
    let body = match message {
        UpstreamMessage::Text(text) => Some(text.as_bytes()),
        UpstreamMessage::Binary(bytes) => Some(bytes.as_ref()),
        _ => None,
    }?;
    if !super::errors::responses_call_id_is_missing(body) {
        return None;
    }
    let in_flight = state.in_flight.as_mut()?;
    if in_flight.client_visible_output || in_flight.legacy_call_id_repair_attempted {
        return None;
    }
    let mut request = in_flight.request.clone();
    if !request.repair_legacy_call_ids() {
        return None;
    }
    in_flight.legacy_call_id_repair_attempted = true;
    Some(request)
}

async fn install_connected(
    downstream: &mut WebSocket,
    upstream: &mut UpstreamWebSocket,
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    state: &mut BridgeState,
    connected: Connected,
) -> bool {
    let Connected {
        upstream: next_upstream,
        initial_messages,
        route,
        request,
        lease,
        attempt,
        started,
        retry_deadline,
    } = connected;
    let _ = upstream
        .send(UpstreamMessage::Close {
            code: UpstreamCloseCode::Normal,
            reason: String::new(),
        })
        .await;
    *upstream = next_upstream;
    install_in_flight(
        runtime,
        state,
        key,
        InFlightInstall {
            request,
            route,
            lease,
            attempt,
            started,
            retry_deadline,
        },
    );
    handle_initial_messages(downstream, runtime, state, initial_messages).await
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
                .map_err(|_| GatewayFailure::transport(state.upstream_origin))?;
        }
        Message::Pong(payload) => {
            upstream
                .send(UpstreamMessage::Pong(payload))
                .await
                .map_err(|_| GatewayFailure::transport(state.upstream_origin))?;
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
            finish_incomplete(runtime, state, error_codes::CLIENT_CANCELLED);
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
    let mut request = ClientRequest::parse_on_connection(
        runtime,
        key,
        headers,
        payload,
        state
            .last_response_id
            .as_deref()
            .zip(state.transient_response_affinity_key.as_deref()),
    )?;
    if let Some(stream_id) = request.stream_id.as_deref() {
        if let Some(active_stream_id) = state.stream_id.as_deref() {
            if active_stream_id != stream_id {
                return Err(GatewayFailure::invalid_request(
                    "only one WebSocket stream_id is supported per connection",
                ));
            }
        } else {
            state.stream_id = Some(stream_id.to_string());
        }
    }
    let same_response_id = request
        .previous_response_id()
        .is_some_and(|response_id| Some(response_id) == state.last_response_id.as_deref());
    let response_affinity_key = request.response_affinity_key.clone();
    let model_switch_reset = same_response_id
        && !request.has_unpaired_tool_output()
        && response_affinity_key.as_deref().and_then(|affinity_key| {
            runtime.response_affinity_owner_supports_route(
                key,
                affinity_key,
                &request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                now_ms(),
            )
        }) == Some(false)
        && request.drop_previous_response_id(runtime, &key.id);
    if model_switch_reset {
        clear_transient_response_affinity(runtime, state);
        state.last_response_id = None;
    }
    if same_response_id && !model_switch_reset {
        let tried = HashSet::new();
        let selected = runtime
            .select_and_reserve(
                key,
                &request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                &tried,
                (
                    request.response_affinity_key.as_deref(),
                    request.prompt_affinity_key.as_deref(),
                ),
                now_ms(),
            )
            .await;
        let Some((selected, lease)) = selected else {
            // The owner can become temporarily ineligible while this client
            // WebSocket stays open because another chat consumed its quota.
            // Reuse would fail before reaching the owner, so reconnect through
            // the normal bounded replay path. That path materializes the local
            // Responses history and removes the opaque owner reference before
            // choosing a compatible account or API source.
            if request.has_previous_response_id() && request.requires_affinity_owner {
                let connected = connect_upstream_while_client_connected(
                    downstream, runtime, key, headers, request, false, 0, None,
                )
                .await?;
                return Ok(
                    install_connected(downstream, upstream, runtime, key, state, connected).await,
                );
            }
            if let Some(retry_at_ms) = runtime.earliest_retry_at(
                key,
                &request.resolved_model,
                WEBSOCKET_PROTOCOLS,
                &tried,
                request.response_affinity_key.as_deref(),
                now_ms(),
            ) {
                return Err(GatewayFailure::cooldown(retry_at_ms));
            }
            return Err(GatewayFailure::unavailable());
        };
        if selected.candidate_id != state.upstream_candidate_id {
            return Err(GatewayFailure::unavailable());
        }
        let mut route = runtime
            .executor_route(
                &selected.candidate_id,
                &request.resolved_model,
                &key.scope_snapshot(),
                WEBSOCKET_PROTOCOLS,
                false,
            )
            .ok_or_else(GatewayFailure::unavailable)?;
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.client_context_id = client_context_fingerprint(headers);
        request.apply_service_tier_for_route(runtime, &route);
        route.service_tier = request.service_tier(runtime, &route);
        let started = Instant::now();
        let upstream_origin = route_error_origin(&route);
        if let Err(failure) =
            send_request(upstream, request.payload_for(&route)?, upstream_origin).await
        {
            record_connect_failure(runtime, key, &route, &request, 1, started, &failure, None);
            return Err(failure);
        }
        let event = usage_event(
            &request.request_id,
            1,
            &key.id,
            &route,
            Some(&request.reasoning_effort_for(&route)),
            &request.requested_model,
            true,
            StatusCode::OK.as_u16(),
            None,
            0,
            request.tool_use_for(&route),
        );
        state.lease = Some(lease);
        state.upstream_origin = upstream_origin;
        state.in_flight = Some(InFlight {
            request: request.clone(),
            route: route.clone(),
            event,
            started,
            response_id: None,
            prompt_affinity_key: request.prompt_affinity_key,
            retry_deadline: (!runtime.chatgpt_retry_until_available()).then(|| {
                TokioInstant::now() + Duration::from_millis(runtime.chatgpt_retry_window_ms())
            }),
            client_visible_output: false,
            legacy_call_id_repair_attempted: false,
            native_replay_capture: NativeReplayCapture::default(),
        });
        return Ok(true);
    }
    let connected = connect_upstream_while_client_connected(
        downstream, runtime, key, headers, request, true, 0, None,
    )
    .await?;
    let Connected {
        upstream: next_upstream,
        initial_messages,
        route,
        request,
        lease,
        attempt,
        started,
        retry_deadline,
    } = connected;
    let _ = upstream
        .send(UpstreamMessage::Close {
            code: UpstreamCloseCode::Normal,
            reason: String::new(),
        })
        .await;
    *upstream = next_upstream;
    install_in_flight(
        runtime,
        state,
        key,
        InFlightInstall {
            request,
            route,
            lease,
            attempt,
            started,
            retry_deadline,
        },
    );
    Ok(handle_initial_messages(downstream, runtime, state, initial_messages).await)
}

async fn handle_initial_messages(
    downstream: &mut WebSocket,
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    initial_messages: Vec<UpstreamMessage>,
) -> bool {
    for message in initial_messages {
        if !handle_upstream_message(downstream, runtime, state, message).await {
            return false;
        }
    }
    true
}

struct InFlightInstall {
    request: ClientRequest,
    route: ExecutorRoute,
    lease: CandidateLease,
    attempt: u16,
    started: Instant,
    retry_deadline: Option<TokioInstant>,
}

fn install_in_flight(
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    key: &AuthenticatedKey,
    install: InFlightInstall,
) {
    let InFlightInstall {
        request,
        route,
        lease,
        attempt,
        started,
        retry_deadline,
    } = install;
    let event = usage_event(
        &request.request_id,
        attempt,
        &key.id,
        &route,
        Some(&request.reasoning_effort_for(&route)),
        &request.requested_model,
        true,
        StatusCode::OK.as_u16(),
        None,
        0,
        request.tool_use_for(&route),
    );
    clear_transient_response_affinity(runtime, state);
    state.lease = Some(lease);
    state.upstream_candidate_id = route.candidate_id.clone();
    state.upstream_origin = route_error_origin(&route);
    state.last_response_id = None;
    state.in_flight = Some(InFlight {
        request: request.clone(),
        route,
        event,
        started,
        response_id: None,
        prompt_affinity_key: request.prompt_affinity_key,
        retry_deadline,
        client_visible_output: false,
        legacy_call_id_repair_attempted: false,
        native_replay_capture: NativeReplayCapture::default(),
    });
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
                let request_id = state.request_id().map(str::to_owned);
                let can_send_error = state.can_send_gateway_error();
                finish_incomplete(runtime, state, error_codes::STREAM_EVENT_TOO_LARGE);
                if can_send_error {
                    send_gateway_error(
                        downstream,
                        &GatewayFailure::message_too_large(state.upstream_origin),
                        request_id.as_deref(),
                    )
                    .await;
                }
                return false;
            }
            let terminal = inspect_upstream_event(text.as_bytes(), state);
            mark_client_visible_output(state, text.as_bytes());
            if downstream.send(Message::Text(text.into())).await.is_err() {
                finish_incomplete(runtime, state, error_codes::CLIENT_CANCELLED);
                return false;
            }
            finish_terminal(runtime, state, terminal)
        }
        UpstreamMessage::Binary(bytes) => {
            if bytes.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
                let request_id = state.request_id().map(str::to_owned);
                let can_send_error = state.can_send_gateway_error();
                finish_incomplete(runtime, state, error_codes::STREAM_EVENT_TOO_LARGE);
                if can_send_error {
                    send_gateway_error(
                        downstream,
                        &GatewayFailure::message_too_large(state.upstream_origin),
                        request_id.as_deref(),
                    )
                    .await;
                }
                return false;
            }
            let terminal = inspect_upstream_event(&bytes, state);
            mark_client_visible_output(state, bytes.as_ref());
            if downstream.send(Message::Binary(bytes)).await.is_err() {
                finish_incomplete(runtime, state, error_codes::CLIENT_CANCELLED);
                return false;
            }
            finish_terminal(runtime, state, terminal)
        }
        UpstreamMessage::Ping(payload) => downstream.send(Message::Ping(payload)).await.is_ok(),
        UpstreamMessage::Pong(payload) => downstream.send(Message::Pong(payload)).await.is_ok(),
        UpstreamMessage::Close { code, reason } => {
            let active_request = state.in_flight.is_some() && state.can_send_gateway_error();
            let request_id = state.request_id().map(str::to_owned);
            finish_incomplete(runtime, state, error_codes::UPSTREAM_WEBSOCKET_CLOSED);
            if active_request {
                send_gateway_error(
                    downstream,
                    &GatewayFailure::closed(state.upstream_origin),
                    request_id.as_deref(),
                )
                .await;
            } else {
                let _ = downstream
                    .send(Message::Close(Some(CloseFrame {
                        code: u16::from(code),
                        reason: reason.into(),
                    })))
                    .await;
            }
            false
        }
    }
}

/// Mark a request as owned by the selected route only after the upstream has
/// emitted semantic response output. Lifecycle/setup events such as
/// `response.created` are still forwarded to the client, but they do not make
/// a pre-output reconnect unsafe. Unknown or malformed frames remain
/// conservative and count as visible output.
fn mark_client_visible_output(state: &mut BridgeState, payload: &[u8]) {
    if semantic_output_payload(payload) {
        if let Some(in_flight) = state.in_flight.as_mut() {
            in_flight.client_visible_output = true;
        }
    }
}

fn semantic_output_payload(payload: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return true;
    };
    let event_type = value.get("type").and_then(Value::as_str);
    if has_semantic_output(&value, event_type) {
        return true;
    }
    // Compaction and known lifecycle notifications are setup/state. A future
    // event is safer to classify as visible than to replay it to a client.
    !is_known_non_output_event(&value, event_type)
}

fn inspect_upstream_event(payload: &[u8], state: &mut BridgeState) -> EventTerminal {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        if let Some(in_flight) = state.in_flight.as_mut() {
            in_flight.native_replay_capture.mark_unmaterialized();
        }
        return EventTerminal::default();
    };
    let event_type = value.get("type").and_then(Value::as_str);
    if let Some(in_flight) = state.in_flight.as_mut() {
        in_flight.native_replay_capture.observe(&value);
        in_flight.event.tool_use.observe_stream_payload(&value);
        if has_output_delta(&value, event_type) && in_flight.event.ttft_ms.is_none() {
            in_flight.event.ttft_ms = Some(in_flight.started.elapsed().as_millis() as u64);
        }
        if let Some(usage) = super::response::find_usage(&value) {
            apply_usage(&mut in_flight.event, usage);
        }
        if let Some(response_id) = super::response::response_id(&value) {
            in_flight.response_id = Some(response_id.to_string());
        }
    }
    event_terminal(&value)
}

fn finish_terminal(
    runtime: &GatewayRuntime,
    state: &mut BridgeState,
    terminal: EventTerminal,
) -> bool {
    let Some(outcome) = terminal.outcome else {
        return true;
    };
    let Some(mut in_flight) = state.in_flight.take() else {
        return true;
    };
    // `response.incomplete` is a terminal event for this request, so the
    // client WebSocket may carry its next independent request. It is not a
    // successful response: do not retain response/session affinity or reset
    // slot health from a partially completed stream.
    let terminal_success = matches!(outcome, EventTerminalOutcome::Success);
    let keep_client_socket = !matches!(outcome, EventTerminalOutcome::Failure);
    in_flight.event.latency_ms = in_flight.started.elapsed().as_millis() as u64;
    in_flight.event.generation_ms = in_flight
        .event
        .ttft_ms
        .map(|ttft_ms| in_flight.event.latency_ms.saturating_sub(ttft_ms))
        .filter(|duration| *duration > 0);
    in_flight.event.success = terminal_success;
    if terminal_success {
        in_flight.event.tool_use.finish();
    }
    if matches!(outcome, EventTerminalOutcome::Incomplete) {
        in_flight.event.error_category = Some(error_codes::RESPONSE_INCOMPLETE.to_string());
    }
    if terminal.deactivated_workspace
        && terminal.status == Some(StatusCode::PAYMENT_REQUIRED)
        && in_flight.route.account_id.is_some()
    {
        runtime.trip_chatgpt_team_breaker(&in_flight.route.candidate_id, now_ms());
    }
    runtime.observe_codex_quota_headers(
        &in_flight.route.candidate_id,
        match outcome {
            EventTerminalOutcome::Success => terminal.status.unwrap_or(StatusCode::OK),
            EventTerminalOutcome::Incomplete => StatusCode::OK,
            EventTerminalOutcome::Failure => terminal.status.unwrap_or(StatusCode::BAD_GATEWAY),
        },
        &terminal.headers,
        now_ms(),
    );
    // A continuation may legitimately reference an incomplete response (for
    // example after max_output_tokens). Keep that id only for this live
    // WebSocket; durable response/prompt affinity still requires success.
    if terminal_success {
        clear_transient_response_affinity(runtime, state);
        state.last_response_id = in_flight.response_id.clone();
    } else if matches!(outcome, EventTerminalOutcome::Incomplete) {
        clear_transient_response_affinity(runtime, state);
        state.last_response_id = in_flight.response_id.clone();
        state.transient_response_affinity_key = runtime.bind_volatile_response_affinity(
            in_flight.response_id.as_deref(),
            &in_flight.route.candidate_id,
            &in_flight.request.request_id,
            now_ms(),
        );
    }
    if terminal_success {
        if let Some(response) = in_flight
            .native_replay_capture
            .finish(terminal.response.clone(), in_flight.response_id.as_deref())
        {
            runtime.capture_native_responses_replay(
                &state.local_key_id,
                &in_flight.route.candidate_id,
                &in_flight.request.native_replay_value(),
                &in_flight.route.source_model,
                &response,
                now_ms(),
            );
        }
        let recovered = runtime.record_success_with_metrics(
            &in_flight.route.candidate_id,
            &in_flight.route.source_model,
            now_ms(),
            in_flight.event.output_tokens,
            in_flight
                .event
                .generation_ms
                .unwrap_or(in_flight.event.latency_ms),
        );
        runtime.bind_response_affinity(
            in_flight.response_id.as_deref(),
            &in_flight.route.candidate_id,
            now_ms(),
        );
        runtime.bind_prompt_affinity(
            in_flight.prompt_affinity_key.as_deref(),
            &in_flight.route.candidate_id,
            now_ms(),
        );
        in_flight.event.consecutive_failures = recovered.then_some(0);
    } else if matches!(outcome, EventTerminalOutcome::Failure) {
        let category = terminal.error_category.unwrap_or_else(|| {
            super::errors::classify_upstream_error(terminal_failure_status(terminal.status), None)
                .category
        });
        let status = terminal
            .status
            .filter(|status| !status.is_success())
            .unwrap_or_else(|| super::errors::upstream_failure_status(category));
        let status = super::errors::canonical_upstream_status(status, category);
        in_flight.event.http_status = status.as_u16();
        in_flight.event.error_category = Some(category.to_string());
        in_flight.event.upstream_error = terminal.upstream_error;
        if super::errors::retryable_failure(status, category, false) {
            let cooldown_context = CooldownContext {
                scope: &in_flight.route.scope,
                allowed_protocols: &in_flight.route.allowed_protocols,
            };
            let failure_state = apply_failure_cooldown_with_hint(
                runtime,
                &in_flight.route.candidate_id,
                &in_flight.route.source_model,
                status,
                category,
                &terminal.headers,
                terminal.body_hint,
                &cooldown_context,
                in_flight.route.half_open_probe,
            );
            apply_failure_state(&mut in_flight.event, failure_state);
        }
    }
    emit_usage(runtime, in_flight.event);
    state.lease.take();
    keep_client_socket
}

fn finish_incomplete(runtime: &GatewayRuntime, state: &mut BridgeState, category: &str) {
    clear_transient_response_affinity(runtime, state);
    let Some(mut in_flight) = state.in_flight.take() else {
        state.lease.take();
        return;
    };
    in_flight.event.success = false;
    in_flight.event.error_category = Some(category.to_string());
    in_flight.event.latency_ms = in_flight.started.elapsed().as_millis() as u64;
    if let Some(status) = incomplete_status(category) {
        in_flight.event.http_status = status.as_u16();
    }
    in_flight.event.generation_ms = in_flight
        .event
        .ttft_ms
        .map(|ttft_ms| in_flight.event.latency_ms.saturating_sub(ttft_ms))
        .filter(|duration| *duration > 0);
    // Cool only this physical slot and model. A stream failure on one
    // credential must not make unrelated routes unavailable, regardless of
    // whether the selected route is an OAuth account or a direct API source.
    if incomplete_requires_cooldown(category) {
        let cooldown_context = CooldownContext {
            scope: &in_flight.route.scope,
            allowed_protocols: &in_flight.route.allowed_protocols,
        };
        let failure_state = apply_cooldown(
            runtime,
            &in_flight.route.candidate_id,
            &in_flight.route.source_model,
            TRANSIENT_COOLDOWN_MS,
            &cooldown_context,
            in_flight.route.half_open_probe,
        );
        apply_failure_state(&mut in_flight.event, failure_state);
    }
    emit_usage(runtime, in_flight.event);
    state.lease.take();
}

fn clear_transient_response_affinity(runtime: &GatewayRuntime, state: &mut BridgeState) {
    if let Some(key) = state.transient_response_affinity_key.take() {
        runtime.invalidate_response_affinity(Some(&key));
    }
}

#[cfg(test)]
mod tests {
    use super::events::{websocket_reset_delay_seconds, websocket_retry_headers};
    use super::{
        event_terminal, fallback_event_message, fallback_response_origin,
        incomplete_requires_cooldown, initial_payloads_are_empty_incomplete,
        semantic_output_payload, terminal_failure_status, ClientRequest, EventTerminalOutcome,
        GatewayFailure, MAX_SSE_EVENT_BYTES, MAX_WEBSOCKET_ERROR_BYTES, RELAY_ERROR_ORIGIN_HEADER,
        RELAY_UPSTREAM_ORIGIN_HEADER, WEBSOCKET_PROTOCOLS,
    };
    use crate::{
        ErrorOrigin, GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey, ProviderSource,
        RuntimeLocalKey, RuntimeSource, WireApi,
    };
    use axum::body::Body;
    use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
    use serde_json::json;
    use std::sync::Arc;

    fn runtime() -> GatewayRuntime {
        GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: "source".into(),
                name: "source".into(),
                base_url: "https://example.test/v1".into(),
                api_key: "upstream-secret".into(),
                wire_api: WireApi::Responses,
                models: vec!["upstream-model".into()],
            })],
            vec![RuntimeLocalKey {
                key: LocalGatewayKey {
                    id: "key".into(),
                    secret: "local-secret".into(),
                },
                enabled: true,
                source_ids: None,
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                model_prefix: Some("relay".into()),
            }],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap()
    }

    #[test]
    fn only_upstream_incomplete_failures_cool_the_candidate() {
        assert!(incomplete_requires_cooldown("upstream_websocket_closed"));
        assert!(incomplete_requires_cooldown("websocket_idle_timeout"));
        assert!(!incomplete_requires_cooldown("client_cancelled"));
        assert!(!incomplete_requires_cooldown("invalid_request"));
    }

    #[test]
    fn response_incomplete_is_terminal_but_not_slot_success() {
        let terminal = event_terminal(&json!({
            "type": "response.incomplete",
            "response": {
                "id": "resp_incomplete",
                "status": "incomplete",
                "incomplete_details": {"reason": "max_output_tokens"}
            }
        }));

        assert_eq!(terminal.outcome, Some(EventTerminalOutcome::Incomplete));
        assert_eq!(terminal.error_category, Some("response_incomplete"));
        assert!(!incomplete_requires_cooldown("response_incomplete"));
    }

    #[test]
    fn websocket_bootstrap_retries_only_empty_zero_token_incomplete() {
        let empty = vec![
            json!({"type": "response.created", "response": {"id": "resp_1"}}),
            json!({
                "type": "response.incomplete",
                "response": {"output": [], "usage": {"output_tokens": 0}}
            }),
        ];
        assert!(initial_payloads_are_empty_incomplete(&empty));

        let with_reasoning = vec![
            json!({"type": "response.reasoning_text.delta", "delta": "thinking"}),
            json!({
                "type": "response.incomplete",
                "response": {"output": [], "usage": {"output_tokens": 0}}
            }),
        ];
        assert!(!initial_payloads_are_empty_incomplete(&with_reasoning));

        let with_completed_item = vec![
            json!({"type": "response.output_item.done", "item": {"type": "message"}}),
            json!({
                "type": "response.incomplete",
                "response": {"output": [], "usage": {"output_tokens": 0}}
            }),
        ];
        assert!(!initial_payloads_are_empty_incomplete(&with_completed_item));

        let non_zero = vec![json!({
            "type": "response.incomplete",
            "response": {"output": [], "usage": {"output_tokens": 1}}
        })];
        assert!(!initial_payloads_are_empty_incomplete(&non_zero));
    }

    #[test]
    fn websocket_setup_events_do_not_commit_route_ownership() {
        assert!(!semantic_output_payload(
            br#"{"type":"response.created","response":{"id":"resp_1"}}"#
        ));
        assert!(!semantic_output_payload(
            br#"{"type":"response.in_progress","response":{"id":"resp_1"}}"#
        ));
        assert!(semantic_output_payload(
            br#"{"type":"response.output_text.delta","delta":"hello"}"#
        ));
        assert!(!semantic_output_payload(
            br#"{"type":"response.compaction.delta","opaque":true}"#
        ));
        assert!(!semantic_output_payload(
            br#"{"type":"response.output_item.done","item":{"type":"compaction","encrypted_content":"opaque"}}"#
        ));
    }

    #[test]
    fn websocket_unknown_or_malformed_frames_are_conservative() {
        assert!(semantic_output_payload(
            br#"{"type":"response.future_output_event"}"#
        ));
        assert!(semantic_output_payload(b"not-json"));
    }

    #[test]
    fn http_fallback_preserves_opaque_compaction_data() {
        let terminal = super::parse_sse_event(
            b"event: response.compaction.delta\ndata: encrypted-compaction-fragment\n\n",
        );
        let message = fallback_event_message(&terminal, ErrorOrigin::Account)
            .ok()
            .flatten()
            .expect("opaque compaction must produce a WebSocket message");

        assert!(!message.semantic_output);
        match message.message {
            axum::extract::ws::Message::Text(text) => {
                assert_eq!(text.to_string(), "encrypted-compaction-fragment")
            }
            other => assert!(
                matches!(other, axum::extract::ws::Message::Text(_)),
                "UTF-8 compaction data should remain a text message"
            ),
        }
    }

    #[test]
    fn http_fallback_does_not_commit_response_setup_events() {
        let terminal = super::parse_sse_event(
            br#"data: {"type":"response.created","response":{"id":"resp_setup"}}

"#,
        );
        let message = fallback_event_message(&terminal, ErrorOrigin::Account)
            .ok()
            .flatten()
            .expect("setup event must still be forwarded");

        assert!(!message.semantic_output);
    }

    #[test]
    fn sse_fallback_limit_is_wider_than_the_error_body_limit() {
        assert_eq!(MAX_WEBSOCKET_ERROR_BYTES, 1024 * 1024);
        assert_eq!(MAX_SSE_EVENT_BYTES, 16 * 1024 * 1024);
        const _: () = assert!(MAX_SSE_EVENT_BYTES > MAX_WEBSOCKET_ERROR_BYTES);
    }

    #[test]
    fn absolute_usage_reset_becomes_retry_delay() {
        let value = json!({
            "type": "error",
            "body": {"error": {"type": "usage_limit_reached", "resets_at": 1_700_000_120}}
        });
        assert_eq!(
            websocket_reset_delay_seconds(&value, 1_700_000_000),
            Some(120)
        );
    }

    #[test]
    fn terminal_errors_never_keep_a_success_status() {
        assert_eq!(
            terminal_failure_status(Some(StatusCode::OK)),
            StatusCode::BAD_GATEWAY
        );
        assert_eq!(
            terminal_failure_status(Some(StatusCode::TOO_MANY_REQUESTS)),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[test]
    fn websocket_terminal_and_handshake_keep_original_failure_details() {
        let value = json!({"type": "response.failed", "response": {"error": {
            "code": "future_constraint", "type": "validation_error", "message": "Invalid field: temperature"
        }}});
        let terminal = event_terminal(&value);
        let details = terminal.upstream_error.unwrap();
        assert_eq!(details.code.as_deref(), Some("future_constraint"));
        assert_eq!(details.http_status, None);
        let failure = GatewayFailure::classified(
            StatusCode::BAD_REQUEST,
            "upstream_invalid_request",
            ErrorOrigin::Account,
        )
        .with_upstream_error(Some(details));
        let event = super::failure::gateway_error_event(&failure, None);
        assert_eq!(event["error"]["message"], "Invalid field: temperature");
        let handshake = GatewayFailure::upstream_status(
            StatusCode::UNPROCESSABLE_ENTITY,
            Some(
                br#"{"error":{"code":"validation_error","message":"Invalid field: temperature"}}"#,
            ),
            ErrorOrigin::Account,
        );
        assert_eq!(handshake.status, StatusCode::BAD_REQUEST);
        assert_eq!(handshake.upstream_error.unwrap().http_status, Some(422));
    }

    #[test]
    fn upstream_status_accepts_string_status_codes() {
        let value = serde_json::json!({"type": "error", "status": "429"});
        assert_eq!(
            crate::gateway::errors::upstream_status_from_value(&value),
            Some(StatusCode::TOO_MANY_REQUESTS)
        );
    }

    #[test]
    fn websocket_retry_headers_preserve_nested_reset_and_quota_hints() {
        let value = json!({
            "body": {
                "error": {"resets_in_seconds": 45},
                "headers": {"x-codex-primary-used-percent": "99"}
            }
        });
        let headers = websocket_retry_headers(&value);

        assert_eq!(
            headers
                .get("retry-after")
                .and_then(|value| value.to_str().ok()),
            Some("45")
        );
        assert_eq!(
            headers
                .get("x-codex-primary-used-percent")
                .and_then(|value| value.to_str().ok()),
            Some("99")
        );
    }

    #[test]
    fn websocket_cooldown_failure_keeps_retry_metadata() {
        let failure = GatewayFailure::cooldown(1_700_000_120_000);

        assert_eq!(failure.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(failure.category, "all_candidates_cooling_down");
        assert_eq!(failure.retry_at_ms, Some(1_700_000_120_000));
        assert_eq!(failure.origin, ErrorOrigin::Relay);
    }

    #[test]
    fn websocket_errors_keep_the_source_origin_and_unmapped_category() {
        let failure = GatewayFailure::classified(
            StatusCode::BAD_REQUEST,
            "upstream_invalid_request",
            ErrorOrigin::Account,
        );
        let event = super::failure::gateway_error_event(&failure, Some("relay-request-3"));

        assert_eq!(event["error"]["code"], "invalid_request");
        assert_eq!(
            event["error"]["zenith_relay"]["category"],
            "upstream_invalid_request"
        );
        assert_eq!(event["error"]["zenith_relay"]["origin"], "account");
        assert_eq!(
            event["error"]["zenith_relay"]["request_id"],
            "relay-request-3"
        );
    }

    #[test]
    fn http_fallback_preserves_provider_and_account_error_origins() {
        let provider = Response::builder()
            .status(StatusCode::TOO_MANY_REQUESTS)
            .header(RELAY_ERROR_ORIGIN_HEADER, "provider")
            .body(Body::empty())
            .unwrap();
        assert_eq!(fallback_response_origin(&provider), ErrorOrigin::Provider);

        let account = Response::builder()
            .status(StatusCode::UNAUTHORIZED)
            .header(RELAY_ERROR_ORIGIN_HEADER, "account")
            .body(Body::empty())
            .unwrap();
        assert_eq!(fallback_response_origin(&account), ErrorOrigin::Account);
    }

    #[test]
    fn http_fallback_rejects_unknown_or_untrusted_error_origins() {
        let unknown = Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header(RELAY_ERROR_ORIGIN_HEADER, "external")
            .body(Body::empty())
            .unwrap();
        assert_eq!(fallback_response_origin(&unknown), ErrorOrigin::Relay);

        let upstream = Response::builder()
            .status(StatusCode::OK)
            .header(RELAY_UPSTREAM_ORIGIN_HEADER, "provider")
            .body(Body::empty())
            .unwrap();
        assert_eq!(fallback_response_origin(&upstream), ErrorOrigin::Provider);
    }

    #[test]
    fn client_request_resolves_the_visible_model_before_upstream_serialization() {
        let runtime = runtime();
        runtime.bind_response_affinity(Some("resp_previous"), "source", crate::unix_time_ms());
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            crate::gateway::request::CODEX_RESPONSES_LITE_HEADER,
            HeaderValue::from_static("true"),
        );
        let request_result = ClientRequest::parse(
            &runtime,
            &key,
            &headers,
            br#"{
                "model": "relay/upstream-model",
                "input": "hello",
                "previous_response_id": "resp_previous",
                "prompt_cache_key": "cache-key"
            }"#,
        );
        assert!(request_result.is_ok(), "request should be accepted");
        let Ok(request) = request_result else {
            return;
        };
        let route = runtime
            .executor_route(
                "source",
                &request.resolved_model,
                &key.scope_snapshot(),
                WEBSOCKET_PROTOCOLS,
                false,
            )
            .unwrap();
        let payload_result = request.payload_for(&route);
        assert!(payload_result.is_ok(), "request should be serializable");
        let Ok(payload_text) = payload_result else {
            return;
        };
        let payload: serde_json::Value = serde_json::from_str(&payload_text).unwrap();

        assert_eq!(request.requested_model, "relay/upstream-model");
        assert_eq!(request.resolved_model, "upstream-model");
        assert!(request.responses_lite);
        assert!(request.response_affinity_key.is_some());
        assert!(request.requires_affinity_owner);
        assert!(request.prompt_affinity_key.is_some());
        assert_eq!(payload["type"], "response.create");
        assert_eq!(payload["model"], "upstream-model");
        assert_eq!(payload["input"], "hello");
    }

    #[test]
    fn client_request_rejects_unknown_previous_response_even_with_plaintext_history() {
        let runtime = runtime();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let request = ClientRequest::parse(
            &runtime,
            &key,
            &HeaderMap::new(),
            br#"{
                "type": "response.create",
                "model": "relay/upstream-model",
                "previous_response_id": "resp_external_history",
                "input": [
                    {"type":"message","role":"user","content":"What is the capital of France?"},
                    {"type":"message","role":"assistant","content":"Paris is the capital of France."},
                    {"type":"message","role":"user","content":"Name one landmark there."}
                ]
            }"#,
        )
        .err().expect("client message shape cannot prove complete history");

        assert_eq!(request.status, StatusCode::CONFLICT);
    }

    #[test]
    fn client_request_rejects_an_unknown_opaque_previous_response_before_selection() {
        let runtime = runtime();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let error = ClientRequest::parse(
            &runtime,
            &key,
            &HeaderMap::new(),
            br#"{
                "type": "response.create",
                "model": "relay/upstream-model",
                "previous_response_id": "resp_external_opaque",
                "input": "continue"
            }"#,
        )
        .err()
        .expect("unknown opaque continuation must not reach selection");

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(error.category, "response_continuation_unavailable");
    }

    #[test]
    fn client_request_rejects_an_unknown_tool_continuation_before_selection() {
        let runtime = runtime();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let error = ClientRequest::parse(
            &runtime,
            &key,
            &HeaderMap::new(),
            br#"{
                "type": "response.create",
                "model": "relay/upstream-model",
                "previous_response_id": "resp_external_tool",
                "input": [{"type":"function_call_output","call_id":"call_external","output":"done"}]
            }"#,
        )
        .err()
        .expect("unknown tool continuation must not reach selection");

        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(error.category, "response_continuation_unavailable");
    }

    #[test]
    fn websocket_lite_contract_is_normalized_before_non_account_routing() {
        let runtime = runtime();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            crate::gateway::request::CODEX_RESPONSES_LITE_HEADER,
            HeaderValue::from_static("true"),
        );
        let request_result = ClientRequest::parse(
            &runtime,
            &key,
            &headers,
            br#"{
                "type": "response.create",
                "model": "relay/upstream-model",
                "input": "hello",
                "parallel_tool_calls": true
            }"#,
        );
        assert!(request_result.is_ok(), "request should be accepted");
        let Ok(request) = request_result else {
            return;
        };
        let route = runtime
            .executor_route(
                "source",
                &request.resolved_model,
                &key.scope_snapshot(),
                WEBSOCKET_PROTOCOLS,
                false,
            )
            .expect("test source should be routable");
        let payload_result = request.payload_for(&route);
        assert!(payload_result.is_ok(), "request should serialize");
        let Ok(payload_bytes) = payload_result else {
            return;
        };
        let payload: serde_json::Value =
            serde_json::from_str(&payload_bytes).expect("payload should be valid JSON");

        assert_eq!(payload["parallel_tool_calls"], false);
        assert!(request.responses_lite_for(&route));
    }

    #[test]
    fn websocket_lite_contract_rejects_non_boolean_parallel_tools() {
        let runtime = runtime();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(
            crate::gateway::request::CODEX_RESPONSES_LITE_HEADER,
            HeaderValue::from_static("true"),
        );
        let parse_result = ClientRequest::parse(
            &runtime,
            &key,
            &headers,
            br#"{
                "type": "response.create",
                "model": "relay/upstream-model",
                "input": "hello",
                "parallel_tool_calls": "yes"
            }"#,
        );
        assert!(
            parse_result.is_err(),
            "non-boolean Lite tool setting must be rejected"
        );
        let error = match parse_result {
            Err(error) => error,
            Ok(_) => return,
        };

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            error.message,
            "responses Lite requires parallel_tool_calls to be a boolean"
        );
    }

    #[test]
    fn client_request_rejects_non_create_messages_before_candidate_selection() {
        let runtime = runtime();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let error = match ClientRequest::parse(
            &runtime,
            &key,
            &HeaderMap::new(),
            br#"{"type":"response.cancel","model":"relay/upstream-model"}"#,
        ) {
            Ok(_) => panic!("non-create message should be rejected"),
            Err(error) => error,
        };

        assert_eq!(error.status, StatusCode::BAD_REQUEST);
        assert_eq!(error.category, "invalid_request");
        assert_eq!(error.message, "only response.create messages are supported");
    }

    #[test]
    fn client_request_accepts_and_preserves_a_stream_id() {
        let runtime = runtime();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let request = match ClientRequest::parse(
            &runtime,
            &key,
            &HeaderMap::new(),
            br#"{"type":"response.create","stream_id":"main","model":"relay/upstream-model","input":"hello"}"#,
        ) {
            Ok(request) => request,
            Err(error) => panic!("stream_id should be accepted: {}", error.message),
        };
        assert_eq!(request.stream_id.as_deref(), Some("main"));
        let route = runtime
            .executor_route(
                "source",
                &request.resolved_model,
                &key.scope_snapshot(),
                WEBSOCKET_PROTOCOLS,
                false,
            )
            .expect("test source should be routable");
        let payload = request
            .payload_for(&route)
            .unwrap_or_else(|error| panic!("payload should serialize: {}", error.message));
        assert!(payload.contains("\"stream_id\":\"main\""));
    }

    #[test]
    fn websocket_request_can_repair_foreign_message_item_ids() {
        let runtime = runtime();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let mut request = ClientRequest::parse(
            &runtime,
            &key,
            &HeaderMap::new(),
            br#"{
                "type": "response.create",
                "model": "relay/upstream-model",
                "input": [
                    {"type":"message","id":"item_foreign","role":"assistant","content":"hello"},
                    {"type":"message","id":"msg_native","role":"user","content":"continue"},
                    {"type":"reasoning","id":"item_reasoning","summary":[]}
                ]
            }"#,
        )
        .unwrap_or_else(|error| panic!("request should be accepted: {}", error.message));

        assert!(request.repair_message_item_ids());
        let route = runtime
            .executor_route(
                "source",
                &request.resolved_model,
                &key.scope_snapshot(),
                WEBSOCKET_PROTOCOLS,
                false,
            )
            .expect("test source should be routable");
        let payload: serde_json::Value = serde_json::from_str(
            &request
                .payload_for(&route)
                .unwrap_or_else(|error| panic!("request should serialize: {}", error.message)),
        )
        .expect("payload should be valid JSON");

        assert!(payload.pointer("/input/0/id").is_none());
        assert_eq!(payload.pointer("/input/1/id"), Some(&json!("msg_native")));
        assert_eq!(
            payload.pointer("/input/2/id"),
            Some(&json!("item_reasoning"))
        );
        assert!(!request.repair_message_item_ids());
    }

    #[test]
    fn websocket_request_repairs_legacy_call_ids_and_recomputes_affinity() {
        let runtime = runtime();
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let request_result = ClientRequest::parse(
            &runtime,
            &key,
            &HeaderMap::new(),
            br#"{
                "type": "response.create",
                "model": "relay/upstream-model",
                "input": [
                    {"type":"function_call_output","output":"orphan"},
                    {"type":"function_call","name":"lookup","arguments":"{}"},
                    {"type":"function_call_output","output":"result"}
                ]
            }"#,
        );
        assert!(request_result.is_ok(), "request should be accepted");
        let Ok(mut request) = request_result else {
            return;
        };

        assert!(!request.has_unpaired_tool_output());
        assert!(request.repair_legacy_call_ids());
        assert!(!request.has_unpaired_tool_output());
        assert!(!request.requires_affinity_owner);

        let route = runtime
            .executor_route(
                "source",
                &request.resolved_model,
                &key.scope_snapshot(),
                WEBSOCKET_PROTOCOLS,
                false,
            )
            .expect("test source should be routable");
        let payload_result = request.payload_for(&route);
        assert!(payload_result.is_ok(), "request should serialize");
        let Ok(payload_bytes) = payload_result else {
            return;
        };
        let payload: serde_json::Value =
            serde_json::from_str(&payload_bytes).expect("payload should be valid JSON");
        let input = payload["input"].as_array().expect("input array");
        assert_eq!(input.len(), 2);
        assert_eq!(input[0]["type"], "function_call");
        assert_eq!(input[0]["call_id"], input[1]["call_id"]);
        assert!(!request.repair_legacy_call_ids());
    }
}

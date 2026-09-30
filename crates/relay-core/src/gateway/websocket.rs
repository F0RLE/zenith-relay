use super::auth::{client_api_forbidden, invalid_host, unauthorized};
use super::errors::{
    apply_failure_state, current_failure_state, rate_limit_body_hint, settle_classified_failure,
    RateLimitBodyHint,
};
use super::execution::{execute_client_request, wait_for_recovery, CandidateRetryContext};
use super::execution::{repair_responses_item_prefixes, ResponsesItemPrefixRepairs};
use super::now_ms;
use super::request::{
    apply_codex_routing_hint, client_context_fingerprint, codex_client_version,
    forwarded_codex_headers, CODEX_RESPONSES_LITE_HEADER,
};
use super::response::{apply_usage, emit_usage, route_error_origin, usage_event, UsageAttempt};
use super::streaming::{
    has_output_delta, has_semantic_output, is_compaction_payload, is_empty_responses_incomplete,
    is_known_non_output_event, parse_sse_event, NativeReplayCapture, MAX_SSE_EVENT_BYTES,
};
use super::turn_state::request_scope;
use crate::error_codes;
use crate::protocol::ClientWireApi;
use crate::runtime::{
    AuthenticatedKey, AuthorizationIncarnation, CandidateLease, ExecutorPrepareError, ExecutorRoute,
};
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

mod bridge;
mod events;
mod failure;
mod fallback;
mod request;
mod upstream;

use bridge::{bridge, semantic_output_payload};
use fallback::bridge_http_fallback;
#[cfg(test)]
use fallback::{
    fallback_event_message, fallback_response_origin, RELAY_ERROR_ORIGIN_HEADER,
    RELAY_UPSTREAM_ORIGIN_HEADER,
};
use upstream::connect_upstream_while_client_connected;
#[cfg(test)]
use upstream::initial_payloads_are_empty_incomplete;

use events::{
    event_terminal, incomplete_requires_cooldown, incomplete_status, resolved_terminal_failure,
    terminal_failure_status, EventTerminal, EventTerminalOutcome,
};
use failure::{send_gateway_error, GatewayFailure};
use request::ClientRequest;

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
        Err((failure, stream_id)) => {
            send_gateway_error(&mut downstream, &failure, None, stream_id.as_deref()).await;
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
            let mut payload = serde_json::json!({
                "type": "response.completed",
                "response": {"id": format!("resp_relay_blocked_{}", request.request_id), "object": "response", "status": "completed", "output": [], "usage": {"input_tokens": 0, "output_tokens": 0, "total_tokens": 0}, "metadata": {"zenith_relay": {"blocked": true, "request_type": kind}}}
            });
            if let Some(stream_id) = request.stream_id.as_deref() {
                payload["stream_id"] = json!(stream_id);
            }
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
    )
    .await
    {
        Ok(connected) => connected,
        Err(failure) if failure.category == error_codes::UPSTREAM_WEBSOCKET_UNSUPPORTED => {
            bridge_http_fallback(downstream, runtime, key, headers, fallback_request).await;
            return;
        }
        Err(failure) => {
            send_gateway_error(
                &mut downstream,
                &failure,
                Some(&request_id),
                fallback_request.stream_id.as_deref(),
            )
            .await;
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
) -> Result<ClientRequest, (GatewayFailure, Option<String>)> {
    let deadline = TokioInstant::now() + INITIAL_MESSAGE_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(TokioInstant::now());
        if remaining.is_zero() {
            return Err((GatewayFailure::request_timeout(), None));
        }
        let message = timeout(remaining, downstream.recv())
            .await
            .map_err(|_| (GatewayFailure::request_timeout(), None))?
            .ok_or_else(|| (GatewayFailure::client_closed(), None))?
            .map_err(|_| {
                (
                    GatewayFailure::invalid_request("invalid WebSocket frame"),
                    None,
                )
            })?;
        match message {
            Message::Text(text) => {
                return ClientRequest::parse(runtime, key, headers, text.as_bytes())
                    .map_err(|failure| (failure, ClientRequest::error_stream_id(text.as_bytes())));
            }
            Message::Binary(bytes) => {
                return ClientRequest::parse(runtime, key, headers, &bytes)
                    .map_err(|failure| (failure, ClientRequest::error_stream_id(&bytes)));
            }
            Message::Ping(payload) => {
                if downstream.send(Message::Pong(payload)).await.is_err() {
                    return Err((GatewayFailure::client_closed(), None));
                }
            }
            Message::Pong(_) => {}
            Message::Close(_) => return Err((GatewayFailure::client_closed(), None)),
        }
    }
}

/// Keeps the client-facing Responses WebSocket usable when every selected
/// provider only exposes HTTP/SSE. The normal HTTP executor remains the single
/// source of routing, adapters, retries, usage, and quota accounting.
#[cfg(test)]
mod tests;

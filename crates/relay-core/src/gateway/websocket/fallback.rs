use super::upstream::await_while_client_connected;
use super::*;

pub(super) async fn bridge_http_fallback(
    mut downstream: WebSocket,
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    headers: HeaderMap,
    mut client_request: ClientRequest,
) {
    let mut stream_id = client_request.stream_id.clone();
    loop {
        let mut client_visible_output = false;
        if let Err(failure) = serve_http_fallback_request(
            &mut downstream,
            runtime.clone(),
            &key,
            &headers,
            &client_request,
            &mut client_visible_output,
        )
        .await
        {
            if !client_visible_output {
                let request_id = Some(client_request.request_id.as_str());
                send_gateway_error(
                    &mut downstream,
                    &failure,
                    request_id,
                    client_request.stream_id.as_deref(),
                )
                .await;
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

        let next_request_result = timeout(WEBSOCKET_IDLE_TIMEOUT, async {
            loop {
                let message = downstream.recv().await?;
                let Ok(message) = message else {
                    return None;
                };
                match message {
                    Message::Text(text) => return Some(Some(text.to_string().into_bytes())),
                    Message::Binary(bytes) => return Some(Some(bytes.to_vec())),
                    Message::Ping(ping_payload) => {
                        if downstream.send(Message::Pong(ping_payload)).await.is_err() {
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
        let Ok(Some(Some(next_request_payload))) = next_request_result else {
            return;
        };
        let next_client_request =
            match ClientRequest::parse(&runtime, &key, &headers, &next_request_payload) {
                Ok(client_request) => client_request,
                Err(failure) => {
                    send_gateway_error(&mut downstream, &failure, None, None).await;
                    return;
                }
            };
        if let Some(next_stream_id) = next_client_request.stream_id.as_deref() {
            if let Some(expected) = stream_id.as_deref() {
                if expected != next_stream_id {
                    let failure = GatewayFailure::invalid_request(
                        "only one WebSocket stream_id is supported per connection",
                    );
                    send_gateway_error(
                        &mut downstream,
                        &failure,
                        Some(&next_client_request.request_id),
                        next_client_request.stream_id.as_deref(),
                    )
                    .await;
                    return;
                }
            } else {
                stream_id = Some(next_stream_id.to_string());
            }
        }
        client_request = next_client_request;
    }
}

async fn serve_http_fallback_request(
    downstream: &mut WebSocket,
    runtime: Arc<GatewayRuntime>,
    _key: &AuthenticatedKey,
    client_headers: &HeaderMap,
    client_request: &ClientRequest,
    client_visible_output: &mut bool,
) -> Result<(), GatewayFailure> {
    let mut headers = client_headers.clone();
    for header_name in [
        "connection",
        "upgrade",
        "sec-websocket-key",
        "sec-websocket-version",
        "sec-websocket-protocol",
        "content-length",
    ] {
        headers.remove(header_name);
    }
    let http_request = Request::builder()
        .method(Method::POST)
        .uri("/v1/responses")
        .header("host", "localhost")
        .body(Body::from(client_request.http_payload()?))
        .map_err(|_| GatewayFailure::invalid_request("request could not be serialized"))
        .map(|mut http_request| {
            *http_request.headers_mut() = headers;
            http_request
        })?;
    let mut http_request = http_request;
    http_request
        .extensions_mut()
        .insert(client_request.tool_policy.clone());
    http_request
        .extensions_mut()
        .insert(super::super::execution::RoutedRequestIdentity {
            request_id: client_request.request_id.clone(),
            budget: client_request.budget.clone(),
            transport: crate::UsageTransport::Websocket,
        });
    let upstream_response = await_while_client_connected(
        downstream,
        execute_client_request(Arc::clone(&runtime), http_request, WireApi::Responses),
    )
    .await?;
    // The HTTP executor already knows whether the selected route belongs to an
    // account or an API provider. Preserve that attribution across the
    // WebSocket bridge instead of turning every fallback response into a Relay
    // error merely because the bridge is the component reading it.
    let response_origin = fallback_response_origin(&upstream_response);
    if !upstream_response.status().is_success() {
        let status = upstream_response.status();
        let error_body = await_while_client_connected(
            downstream,
            axum::body::to_bytes(upstream_response.into_body(), MAX_WEBSOCKET_ERROR_BYTES),
        )
        .await?
        .ok();
        return Err(GatewayFailure::upstream_status(
            status,
            error_body.as_deref(),
            response_origin,
        )
        .apply_degraded_route_policy(&runtime));
    }

    let stream_origin = response_origin;

    let mut response_body_stream = upstream_response.into_body().into_data_stream();
    let mut pending = Vec::new();
    while let Some(chunk) =
        await_while_client_connected(downstream, response_body_stream.next()).await?
    {
        let chunk = chunk.map_err(|_| GatewayFailure::transport(stream_origin))?;
        pending.extend_from_slice(&chunk);
        while let Some(event) = crate::protocol::take_sse_event(&mut pending) {
            let terminal = parse_sse_event(&event);
            if terminal.has_data && !terminal.valid {
                return Err(GatewayFailure::transport(stream_origin));
            }
            // An unframed [DONE] carries no Responses terminal payload for a
            // WebSocket client, even if an HTTP adapter treated it as complete.
            if terminal.outcome.is_some() && terminal.event_payload.is_none() {
                return Err(GatewayFailure::closed(stream_origin));
            }
            if let Some(fallback_message) = fallback_event_message(
                &terminal,
                client_request.stream_id.as_deref(),
                stream_origin,
            )? {
                *client_visible_output |= fallback_message.semantic_output;
                downstream
                    .send(fallback_message.message)
                    .await
                    .map_err(|_| GatewayFailure::client_closed())?;
            }
            if terminal.outcome.is_some() {
                return Ok(());
            }
        }
    }
    Err(GatewayFailure::closed(stream_origin))
}

pub(super) struct FallbackEventMessage {
    pub(super) message: Message,
    pub(super) semantic_output: bool,
}

pub(super) fn fallback_event_message(
    terminal: &super::super::streaming::TerminalEvent,
    stream_id: Option<&str>,
    origin: ErrorOrigin,
) -> Result<Option<FallbackEventMessage>, GatewayFailure> {
    if let Some(stream_id) = stream_id {
        let Some(mut stream_payload) = terminal.event_payload.clone() else {
            // A named lane requires JSON events so the lane can be identified.
            // An opaque, non-JSON compaction event cannot be routed safely.
            return if terminal.raw_data.is_some() {
                Err(GatewayFailure::transport(origin))
            } else {
                Ok(None)
            };
        };
        prefix_fallback_error(&mut stream_payload, terminal, origin);
        let Some(payload_object) = stream_payload.as_object_mut() else {
            return Err(GatewayFailure::transport(origin));
        };
        payload_object.insert(
            "stream_id".to_string(),
            Value::String(stream_id.to_string()),
        );
        let encoded_fallback_payload =
            serde_json::to_vec(&stream_payload).map_err(|_| GatewayFailure::transport(origin))?;
        if encoded_fallback_payload.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
            return Err(GatewayFailure::message_too_large(origin));
        }
        let semantic_output =
            !terminal.is_compaction && semantic_output_payload(&encoded_fallback_payload);
        let text = String::from_utf8(encoded_fallback_payload)
            .map_err(|_| GatewayFailure::transport(origin))?;
        return Ok(Some(FallbackEventMessage {
            message: Message::Text(text.into()),
            semantic_output,
        }));
    }
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

    let Some(fallback_payload) = terminal.event_payload.as_ref() else {
        return Ok(None);
    };
    let mut fallback_payload = fallback_payload.clone();
    prefix_fallback_error(&mut fallback_payload, terminal, origin);
    let encoded_fallback_payload =
        serde_json::to_vec(&fallback_payload).map_err(|_| GatewayFailure::transport(origin))?;
    if encoded_fallback_payload.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
        return Err(GatewayFailure::message_too_large(origin));
    }
    let semantic_output = semantic_output_payload(&encoded_fallback_payload);
    let text = String::from_utf8(encoded_fallback_payload)
        .map_err(|_| GatewayFailure::transport(origin))?;
    Ok(Some(FallbackEventMessage {
        message: Message::Text(text.into()),
        semantic_output,
    }))
}

fn prefix_fallback_error(
    fallback_payload: &mut Value,
    terminal: &super::super::streaming::TerminalEvent,
    origin: ErrorOrigin,
) {
    if !matches!(
        terminal.outcome,
        Some(
            super::super::streaming::TerminalOutcome::Failure
                | super::super::streaming::TerminalOutcome::Incomplete
        )
    ) {
        return;
    }
    let category = terminal
        .error_category
        .unwrap_or(crate::error_codes::UPSTREAM_TERMINAL);
    super::super::errors::prefix_error_value(fallback_payload, origin.for_category(category));
}

pub(super) const RELAY_ERROR_ORIGIN_HEADER: &str = "x-zenith-relay-error-origin";
pub(super) const RELAY_UPSTREAM_ORIGIN_HEADER: &str = "x-zenith-relay-upstream-origin";

pub(super) fn fallback_response_origin(upstream_response: &Response<Body>) -> ErrorOrigin {
    upstream_response
        .headers()
        .get(RELAY_ERROR_ORIGIN_HEADER)
        .or_else(|| {
            upstream_response
                .headers()
                .get(RELAY_UPSTREAM_ORIGIN_HEADER)
        })
        .and_then(|origin_header| origin_header.to_str().ok())
        .and_then(|origin_text| origin_text.trim().parse().ok())
        .unwrap_or(ErrorOrigin::Relay)
}

pub(super) fn websocket_transport_fallback_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::BAD_REQUEST
            | StatusCode::NOT_FOUND
            | StatusCode::METHOD_NOT_ALLOWED
            | StatusCode::UPGRADE_REQUIRED
            | StatusCode::NOT_IMPLEMENTED
    )
}

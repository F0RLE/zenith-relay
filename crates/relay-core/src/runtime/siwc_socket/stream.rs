use super::*;
use axum::body::Bytes;
use reqwest::ResponseBuilderExt;
use serde_json::Value;

impl PendingSocket {
    pub(crate) async fn send(mut self) -> Result<Response, AuthorizedRequestError> {
        let payload = std::mem::take(&mut self.plan.payload);
        if tokio::time::timeout(
            Duration::from_secs(30),
            self.connection.socket.send_request(payload),
        )
        .await
        .map_or(true, |result| result.is_err())
        {
            return Err(AuthorizedRequestError::WebsocketSend);
        }
        let diagnostics = self.diagnostics.clone();
        let payload_diagnostics = self.payload_diagnostics.clone();
        let url = self.url.clone();
        let stream = futures_util::stream::try_unfold(Some(self), |state| async move {
            let Some(mut state) = state else {
                return Ok(None);
            };
            loop {
                // Match native generation: reasoning has no arbitrary idle
                // deadline. Dropping the downstream body cancels this read.
                let message = state
                    .connection
                    .socket
                    .next()
                    .await
                    .ok_or_else(|| std::io::Error::other("SIWC socket closed before terminal"))?
                    .map_err(|_| std::io::Error::other("SIWC WebSocket read failed"))?;
                let text = match message {
                    Message::Text(text) => text,
                    Message::Ping(_) | Message::Pong(_) => {
                        tokio::time::timeout(CONNECT_TIMEOUT, state.connection.socket.flush())
                            .await
                            .map_err(|_| std::io::Error::other("SIWC heartbeat timed out"))?
                            .map_err(|_| std::io::Error::other("SIWC heartbeat failed"))?;
                        continue;
                    }
                    _ => return Err(std::io::Error::other("SIWC socket ended before terminal")),
                };
                let event = crate::strict_json::parse(&text)
                    .map_err(|_| std::io::Error::other("Invalid SIWC response event"))?;
                state.observe_response_identity(&event)?;
                let kind = event
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(|| std::io::Error::other("Missing SIWC event type"))?;
                if !kind
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_'))
                {
                    return Err(std::io::Error::other("Invalid SIWC event type"));
                }
                state
                    .completed_output
                    .validate_for_replay(&event, state.plan.source_model.as_deref());
                let terminal = matches!(
                    kind,
                    "response.completed"
                        | "response.incomplete"
                        | "response.failed"
                        | "response.cancelled"
                        | "response.canceled"
                        | "error"
                );
                let bytes = Bytes::from(format!("event: {kind}\ndata: {event}\n\n"));
                if !terminal {
                    state.completed_output.observe(&event);
                    return Ok(Some((bytes, Some(state))));
                }
                state.lane.reset_reason = Some(if kind == "response.completed" {
                    ConnectionResetReason::UnprovenOutput
                } else {
                    ConnectionResetReason::ResponseFailed
                });
                if kind == "response.completed"
                    && let Some(response) = state
                        .completed_output
                        .finish(event.get("response").cloned(), state.response_id.as_deref())
                    && let Some(checkpoint) = state.plan.state.complete(&response)
                {
                    state.connection.checkpoint = Some(checkpoint);
                    state.lane.touched = Instant::now();
                    state.lane.reset_reason = None;
                    state.lane.idle = Some(idle::IdleSocket::retain(state.connection));
                }
                return Ok(Some((bytes, None)));
            }
        });
        let mut response: Response = axum::http::Response::builder()
            .status(StatusCode::OK)
            .url(url)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .body(reqwest::Body::wrap_stream(stream))
            .expect("valid SIWC SSE response")
            .into();
        response.extensions_mut().insert(diagnostics);
        if let Some(payload) = payload_diagnostics {
            response.extensions_mut().insert(payload);
        }
        Ok(response)
    }

    fn observe_response_identity(&mut self, event: &Value) -> std::io::Result<()> {
        for id in [
            event.get("response_id"),
            event
                .get("response")
                .and_then(|response| response.get("id")),
        ]
        .into_iter()
        .flatten()
        {
            let id = id
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 256)
                .ok_or_else(|| std::io::Error::other("Invalid SIWC response identity"))?;
            if self
                .response_id
                .as_ref()
                .is_some_and(|expected| expected != id)
            {
                return Err(std::io::Error::other(
                    "SIWC response identity changed during generation",
                ));
            }
            self.response_id.get_or_insert_with(|| id.to_owned());
        }
        Ok(())
    }
}

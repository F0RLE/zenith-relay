use super::*;
use axum::body::Bytes;
use reqwest::ResponseBuilderExt;
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SiwcStreamError {
    Eof,
    PeerClosed,
    Read,
    UnexpectedFrame,
    HeartbeatTimeout,
    Heartbeat,
    InvalidJson,
    InvalidEventType,
    InvalidResponseId,
    ResponseIdChanged,
}

impl SiwcStreamError {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Eof => "eof_before_terminal",
            Self::PeerClosed => "close_before_terminal",
            Self::Read => "read_failed",
            Self::UnexpectedFrame => "unexpected_frame",
            Self::HeartbeatTimeout => "heartbeat_timeout",
            Self::Heartbeat => "heartbeat_failed",
            Self::InvalidJson => "invalid_event_json",
            Self::InvalidEventType => "invalid_event_type",
            Self::InvalidResponseId => "invalid_response_id",
            Self::ResponseIdChanged => "response_id_changed",
        }
    }
}

impl std::fmt::Display for SiwcStreamError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.label())
    }
}

impl std::error::Error for SiwcStreamError {}

impl PendingSocket {
    pub(crate) async fn send(mut self) -> Result<Response, AuthorizedRequestError> {
        let payload = std::mem::take(&mut self.plan.payload);
        let Ok(Ok(sent)) = tokio::time::timeout(
            Duration::from_secs(30),
            self.connection.socket.send_request(payload),
        )
        .await
        else {
            return Err(AuthorizedRequestError::WebsocketSend);
        };
        if let Some(payload) = self.payload_diagnostics.as_mut() {
            payload.body_bytes = sent.body_bytes;
            payload.content_encoding = sent.content_encoding;
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
                    .ok_or(SiwcStreamError::Eof)?
                    .map_err(|_| SiwcStreamError::Read)?;
                let text = match message {
                    Message::Text(text) => text,
                    Message::Ping(_) | Message::Pong(_) => {
                        tokio::time::timeout(CONNECT_TIMEOUT, state.connection.socket.flush())
                            .await
                            .map_err(|_| SiwcStreamError::HeartbeatTimeout)?
                            .map_err(|_| SiwcStreamError::Heartbeat)?;
                        continue;
                    }
                    Message::Close { .. } => return Err(SiwcStreamError::PeerClosed),
                    Message::Binary(_) => return Err(SiwcStreamError::UnexpectedFrame),
                };
                let event =
                    crate::strict_json::parse(&text).map_err(|_| SiwcStreamError::InvalidJson)?;
                state.observe_response_identity(&event)?;
                let kind = event
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or(SiwcStreamError::InvalidEventType)?;
                if !kind
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_'))
                {
                    return Err(SiwcStreamError::InvalidEventType);
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

    fn observe_response_identity(&mut self, event: &Value) -> Result<(), SiwcStreamError> {
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
                .ok_or(SiwcStreamError::InvalidResponseId)?;
            if self
                .response_id
                .as_ref()
                .is_some_and(|expected| expected != id)
            {
                return Err(SiwcStreamError::ResponseIdChanged);
            }
            self.response_id.get_or_insert_with(|| id.to_owned());
        }
        Ok(())
    }
}

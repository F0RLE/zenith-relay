use super::{GatewayFailure, MAX_WEBSOCKET_MESSAGE_BYTES, Message};
use axum::extract::ws::WebSocket as RawWebSocket;
use futures_util::Sink;
use serde::Deserialize;
use serde_json::value::RawValue;
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

pub(super) const MAX_QUEUED_REQUESTS: usize = 32;
pub(super) const MAX_QUEUED_BYTES: usize = MAX_WEBSOCKET_MESSAGE_BYTES;

#[derive(Deserialize)]
struct RequestEnvelope<'a> {
    #[serde(borrow, rename = "type")]
    kind: Option<&'a RawValue>,
}

#[derive(Default)]
struct PendingRequests {
    messages: VecDeque<PendingRequest>,
    bytes: usize,
}

struct PendingRequest {
    message: Message,
    lane: Option<String>,
    needs_idle: bool,
}

impl PendingRequests {
    fn push(&mut self, message: Message) -> Result<bool, GatewayFailure> {
        self.push_with_policy(message, false)
    }

    fn push_with_policy(
        &mut self,
        message: Message,
        needs_idle: bool,
    ) -> Result<bool, GatewayFailure> {
        let payload = match &message {
            Message::Text(text) => text.as_bytes(),
            Message::Binary(bytes) => bytes.as_ref(),
            _ => return Ok(false),
        };
        if payload.len() > MAX_WEBSOCKET_MESSAGE_BYTES {
            return Err(GatewayFailure::invalid_request(
                "WebSocket request is too large",
            ));
        }
        if payload.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
            return Err(GatewayFailure::invalid_request(
                "request must be a JSON object",
            ));
        }
        let envelope: RequestEnvelope<'_> = serde_json::from_slice(payload)
            .map_err(|_| GatewayFailure::invalid_request("request must be a valid JSON object"))?;
        if envelope
            .kind
            .and_then(|kind| serde_json::from_str::<String>(kind.get()).ok())
            .is_some_and(|kind| kind != "response.create")
        {
            return Ok(false);
        }
        if self.messages.len() >= MAX_QUEUED_REQUESTS
            || payload.len() > MAX_QUEUED_BYTES.saturating_sub(self.bytes)
        {
            return Err(GatewayFailure::invalid_request(
                "WebSocket pending request queue is full",
            ));
        }
        self.bytes += payload.len();
        self.messages.push_back(PendingRequest {
            lane: super::ClientRequest::error_stream_id(payload),
            message,
            needs_idle,
        });
        Ok(true)
    }

    fn pop(&mut self) -> Option<Message> {
        self.remove(0)
    }

    fn pop_ready(&mut self, idle: bool, ready: impl Fn(Option<&str>) -> bool) -> Option<Message> {
        if !idle && self.messages.iter().any(|entry| entry.needs_idle) {
            return None;
        }
        let mut blocked = std::collections::BTreeSet::new();
        let index = self.messages.iter().position(|entry| {
            let first = blocked.insert(entry.lane.as_deref());
            first && (!entry.needs_idle || idle) && ready(entry.lane.as_deref())
        })?;
        self.remove(index)
    }

    fn remove(&mut self, index: usize) -> Option<Message> {
        let message = self.messages.remove(index)?.message;
        self.bytes -= match &message {
            Message::Text(text) => text.len(),
            Message::Binary(bytes) => bytes.len(),
            _ => unreachable!("only request data frames are queued"),
        };
        Some(message)
    }
}

/// Own pending client input across native connection retries and HTTP fallback.
/// Preparing a queued request waits until its lane can start.
pub(super) struct WebSocket {
    socket: Socket,
    pending: PendingRequests,
}

enum Socket {
    Raw(Box<RawWebSocket>),
    Lane(Box<super::multiplex::LaneSocket>),
}

impl WebSocket {
    pub(super) fn new(socket: RawWebSocket) -> Self {
        Self {
            socket: Socket::Raw(Box::new(socket)),
            pending: PendingRequests::default(),
        }
    }

    pub(super) fn from_lane(socket: super::multiplex::LaneSocket) -> Self {
        Self {
            socket: Socket::Lane(Box::new(socket)),
            pending: PendingRequests::default(),
        }
    }

    pub(super) fn is_http_lane(&self) -> bool {
        matches!(self.socket, Socket::Lane(_))
    }

    pub(super) async fn note_request_prepared(&mut self) -> Result<(), ()> {
        match &mut self.socket {
            Socket::Raw(_) => Ok(()),
            Socket::Lane(socket) => socket.note_prepared().await,
        }
    }

    pub(super) async fn recv(&mut self) -> Option<Result<Message, axum::Error>> {
        if let Some(message) = self.pending.pop() {
            return Some(Ok(message));
        }
        match &mut self.socket {
            Socket::Raw(socket) => socket.recv().await,
            Socket::Lane(socket) => socket.recv().await,
        }
    }

    pub(super) async fn recv_live(&mut self) -> Option<Result<Message, axum::Error>> {
        match &mut self.socket {
            Socket::Raw(socket) => socket.recv().await,
            Socket::Lane(socket) => socket.recv_live().await,
        }
    }

    pub(super) async fn recv_native(
        &mut self,
        idle: bool,
        ready: impl Fn(Option<&str>) -> bool,
    ) -> Option<(Result<Message, axum::Error>, bool)> {
        if let Some(message) = self.pending.pop_ready(idle, ready) {
            return Some((Ok(message), true));
        }
        self.recv_live().await.map(|message| (message, false))
    }

    pub(super) fn has_pending_lane(&self, lane: Option<&str>) -> bool {
        self.pending
            .messages
            .iter()
            .any(|entry| entry.lane.as_deref() == lane)
    }

    pub(super) fn waits_for_idle(&self) -> bool {
        self.pending.messages.iter().any(|entry| entry.needs_idle)
    }

    pub(super) async fn defer_until_idle(&mut self, bytes: &[u8]) -> Result<(), GatewayFailure> {
        self.defer_with_policy(&Message::Binary(bytes.to_vec().into()), true)
            .await?;
        Ok(())
    }

    pub(super) async fn defer_request(
        &mut self,
        message: &Message,
    ) -> Result<bool, GatewayFailure> {
        self.defer_with_policy(message, false).await
    }

    async fn defer_with_policy(
        &mut self,
        message: &Message,
        needs_idle: bool,
    ) -> Result<bool, GatewayFailure> {
        let queued = if needs_idle {
            self.pending.push_with_policy(message.clone(), true)
        } else {
            self.pending.push(message.clone())
        };
        match queued {
            Ok(queued) => Ok(queued),
            Err(failure) => {
                let payload = match message {
                    Message::Text(text) => text.as_bytes(),
                    Message::Binary(bytes) => bytes.as_ref(),
                    _ => unreachable!("only data frames can fail queue admission"),
                };
                let stream_id = (payload.len() <= MAX_WEBSOCKET_MESSAGE_BYTES)
                    .then(|| super::ClientRequest::error_stream_id(payload))
                    .flatten();
                super::failure::send_request_error(self, &failure, None, stream_id.as_deref())
                    .await
                    .map_err(|_| GatewayFailure::client_closed())?;
                Ok(true)
            }
        }
    }
}

impl Sink<Message> for WebSocket {
    type Error = axum::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match &mut self.socket {
            Socket::Raw(socket) => Pin::new(socket.as_mut()).poll_ready(cx),
            Socket::Lane(socket) => Pin::new(socket.as_mut()).poll_ready(cx),
        }
    }

    fn start_send(mut self: Pin<&mut Self>, message: Message) -> Result<(), Self::Error> {
        match &mut self.socket {
            Socket::Raw(socket) => Pin::new(socket.as_mut()).start_send(message),
            Socket::Lane(socket) => Pin::new(socket.as_mut()).start_send(message),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match &mut self.socket {
            Socket::Raw(socket) => Pin::new(socket.as_mut()).poll_flush(cx),
            Socket::Lane(socket) => Pin::new(socket.as_mut()).poll_flush(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match &mut self.socket {
            Socket::Raw(socket) => Pin::new(socket.as_mut()).poll_close(cx),
            Socket::Lane(socket) => Pin::new(socket.as_mut()).poll_close(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(index: usize) -> Message {
        Message::Text(format!(r#"{{"type":"response.create","input":{index}}}"#).into())
    }

    #[test]
    fn pending_requests_preserve_fifo_and_release_their_byte_budget() {
        let mut pending = PendingRequests::default();
        for index in 0..MAX_QUEUED_REQUESTS {
            assert!(matches!(pending.push(request(index)), Ok(true)));
        }
        assert!(pending.push(request(MAX_QUEUED_REQUESTS)).is_err());
        for index in 0..MAX_QUEUED_REQUESTS {
            assert_eq!(pending.pop(), Some(request(index)));
        }
        assert!(pending.pop().is_none());
        assert_eq!(pending.bytes, 0);
        assert!(matches!(pending.push(request(0)), Ok(true)));
    }

    #[test]
    fn native_queue_skips_busy_lanes_but_never_overtakes_an_idle_only_head() {
        let mut pending = PendingRequests::default();
        let frame = |lane: &str, index: usize| {
            Message::Text(format!(r#"{{"stream_id":"{lane}","input":{index}}}"#).into())
        };
        assert!(matches!(
            pending.push_with_policy(frame("a", 0), true),
            Ok(true)
        ));
        assert!(matches!(pending.push(frame("a", 1)), Ok(true)));
        assert!(matches!(pending.push(frame("b", 2)), Ok(true)));
        assert!(pending.pop_ready(false, |_| true).is_none());
        assert_eq!(pending.pop_ready(true, |_| true), Some(frame("a", 0)));
        assert_eq!(
            pending.pop_ready(false, |lane| lane == Some("b")),
            Some(frame("b", 2))
        );
        assert_eq!(pending.pop_ready(false, |_| true), Some(frame("a", 1)));
        assert_eq!(pending.bytes, 0);
        assert!(matches!(pending.push(frame("a", 3)), Ok(true)));
        assert!(matches!(pending.push(request(4)), Ok(true)));
        assert_eq!(
            pending.pop_ready(false, |lane| lane.is_none()),
            Some(request(4))
        );
        assert_eq!(pending.pop(), Some(frame("a", 3)));
        assert_eq!(pending.bytes, 0);
    }

    #[test]
    fn controls_and_invalid_frames_do_not_enter_the_request_queue() {
        let mut pending = PendingRequests::default();
        for message in [
            Message::Ping(Default::default()),
            Message::Close(None),
            Message::Text(r#"{"type":"response.interrupt"}"#.into()),
        ] {
            assert!(matches!(pending.push(message), Ok(false)));
        }
        for payload in ["invalid", "[]", "null"] {
            assert!(pending.push(Message::Text(payload.into())).is_err());
        }
        assert!(pending.messages.is_empty());
        assert_eq!(pending.bytes, 0);
    }

    #[test]
    fn pending_requests_bound_bytes_and_preserve_native_wire_data() {
        let mut pending = PendingRequests::default();
        let prefix = r#"{ "type":"response.create", "precise":9007199254740993, "input":""#;
        let payload = format!(
            "{prefix}{}\"}}",
            "x".repeat(MAX_QUEUED_BYTES - prefix.len() - 2)
        );
        let frame = Message::Binary(payload.into_bytes().into());
        assert!(matches!(pending.push(frame.clone()), Ok(true)));
        assert_eq!(pending.bytes, MAX_QUEUED_BYTES);
        assert!(pending.push(request(1)).is_err());
        assert_eq!(pending.pop(), Some(frame));
        assert_eq!(pending.bytes, 0);
    }
}

use super::SentPayload;
use crate::usage::RequestContentEncoding;
use futures_util::{Sink, SinkExt, Stream};
use reqwest::header::{HeaderMap, HeaderValue, SEC_WEBSOCKET_EXTENSIONS};
use reqwest_websocket::Message;
use std::{
    pin::Pin,
    task::{Context, Poll, ready},
};
use yawc::{
    Options, Role, WebSocket, WebSocketError,
    frame::{Frame, OpCode},
};

mod meter;

const EXTENSION: &str = "permessage-deflate";
const CLIENT_NO_TAKEOVER: &str = "client_no_context_takeover";
const SERVER_NO_TAKEOVER: &str = "server_no_context_takeover";
// Let the server keep its dictionary. Requesting server_no_context_takeover
// would require the peer to echo it to accept this offer (RFC 7692 §7.1.1.1).
const OFFER: &str = "permessage-deflate; client_no_context_takeover";
const BOTH_NO_TAKEOVER: &str =
    "permessage-deflate; client_no_context_takeover; server_no_context_takeover";
pub(super) const COMPRESSION_THRESHOLD: usize = 64 * 1024;

pub(super) fn offer() -> HeaderValue {
    HeaderValue::from_static(OFFER)
}

pub(super) fn accepted(headers: &HeaderMap, offered: bool) -> Result<Option<&'static str>, ()> {
    fn trim(value: &str) -> &str {
        value.trim_matches([' ', '\t'])
    }
    fn valid_window(value: &str) -> bool {
        let value = trim(value);
        let value = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .unwrap_or(value);
        matches!(value, "8" | "9" | "10" | "11" | "12" | "13" | "14" | "15")
    }
    let all = headers.get_all(SEC_WEBSOCKET_EXTENSIONS);
    let mut headers = all.iter();
    let Some(extension) = headers.next() else {
        return Ok(None);
    };
    if !offered || headers.next().is_some() {
        return Err(());
    }
    let extension = extension.to_str().map_err(|_| ())?;
    let mut params = extension.split(';');
    if params.next().map(trim) != Some(EXTENSION) {
        return Err(());
    }
    let mut client = false;
    let mut server = false;
    let mut server_window = false;
    for param in params {
        // A smaller server window requires no change to our 32 KiB decoder.
        // The peer cannot select a client window we did not offer.
        let param = trim(param);
        let seen = match param.split_once('=') {
            None if param == CLIENT_NO_TAKEOVER => &mut client,
            None if param == SERVER_NO_TAKEOVER => &mut server,
            Some((key, value)) if trim(key) == "server_max_window_bits" && valid_window(value) => {
                &mut server_window
            }
            _ => return Err(()),
        };
        if std::mem::replace(seen, true) {
            return Err(());
        }
    }
    // Honor our client no-context-takeover hint even when it was not echoed.
    Ok(Some(if server { BOTH_NO_TAKEOVER } else { OFFER }))
}

pub(super) struct Socket {
    inner: WebSocket<meter::MeteredIo>,
    meter: meter::Meter,
}

impl Socket {
    pub(super) fn new(upgraded: reqwest::Upgraded, extension: &str) -> Result<Self, ()> {
        let max = super::super::MAX_WEBSOCKET_MESSAGE_BYTES + 1;
        let options = Options::default()
            .with_low_latency_compression()
            .with_compression_threshold(COMPRESSION_THRESHOLD)
            .with_utf8()
            .with_limits(max, max)
            // Leave room for expansion while keeping large messages fragmented.
            .with_max_fragment_size(super::super::io::MAX_OUTBOUND_FRAME_BYTES / 2);
        let (io, meter) = meter::MeteredIo::new(upgraded);
        let inner =
            WebSocket::from_stream_with_extensions(io, Role::Client, Some(extension), options)
                .map_err(|_| ())?;
        Ok(Self { inner, meter })
    }

    pub(super) async fn send_request(&mut self, payload: String) -> Result<SentPayload, ()> {
        let before = self.meter.stats();
        self.send(Message::Text(payload)).await?;
        let after = self.meter.stats();
        Ok(SentPayload {
            body_bytes: after.body_bytes - before.body_bytes,
            content_encoding: if after.compressed_messages != before.compressed_messages {
                RequestContentEncoding::PerMessageDeflate
            } else {
                RequestContentEncoding::Identity
            },
        })
    }
}

impl Stream for Socket {
    type Item = Result<Message, ()>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(match ready!(self.inner.poll_next_frame(cx)) {
            Ok(frame) => Some(receive(frame)),
            Err(WebSocketError::ConnectionClosed) => None,
            Err(_) => Some(Err(())),
        })
    }
}

impl Sink<Message> for Socket {
    type Error = ();

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), ()>> {
        Pin::new(&mut self.inner).poll_ready(cx).map_err(|_| ())
    }

    fn start_send(mut self: Pin<&mut Self>, message: Message) -> Result<(), ()> {
        let frame = match message {
            Message::Text(text) => Frame::text(text),
            Message::Binary(bytes) => Frame::binary(bytes),
            Message::Ping(bytes) => Frame::ping(bytes),
            Message::Pong(bytes) => Frame::pong(bytes),
            Message::Close { code, reason } => Frame::close(u16::from(code).into(), reason),
        };
        Pin::new(&mut self.inner).start_send(frame).map_err(|_| ())
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), ()>> {
        // yawc drains automatic control replies during poll_ready.
        ready!(self.as_mut().poll_ready(cx))?;
        Pin::new(&mut self.inner).poll_flush(cx).map_err(|_| ())
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), ()>> {
        Pin::new(&mut self.inner).poll_close(cx).map_err(|_| ())
    }
}

fn receive(frame: Frame) -> Result<Message, ()> {
    Ok(match frame.opcode() {
        OpCode::Text => Message::Text(std::str::from_utf8(frame.payload()).map_err(|_| ())?.into()),
        OpCode::Binary => Message::Binary(frame.into_payload()),
        OpCode::Ping => Message::Ping(frame.into_payload()),
        OpCode::Pong => Message::Pong(frame.into_payload()),
        OpCode::Close => Message::Close {
            code: frame
                .close_code()
                .map_or_else(Default::default, |code| u16::from(code).into()),
            reason: frame
                .close_reason()
                .map_err(|_| ())?
                .unwrap_or_default()
                .into(),
        },
        OpCode::Continuation => return Err(()),
    })
}

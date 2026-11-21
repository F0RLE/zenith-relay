use futures_util::{Sink, Stream, StreamExt};
use reqwest::header::{
    CONNECTION, SEC_WEBSOCKET_ACCEPT, SEC_WEBSOCKET_EXTENSIONS, SEC_WEBSOCKET_KEY,
    SEC_WEBSOCKET_PROTOCOL, SEC_WEBSOCKET_VERSION, UPGRADE,
};
use reqwest::{Response, StatusCode, Version};
use reqwest_websocket::Message;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio_util::compat::TokioAsyncReadCompatExt;
use tungstenite::handshake::{client::generate_key, derive_accept_key};
use tungstenite::protocol::{Role, WebSocketConfig};

mod deflate;

type Socket = async_tungstenite::WebSocketStream<tokio_util::compat::Compat<reqwest::Upgraded>>;

/// Retains Relay's reqwest client, proxy and TLS policy while exposing frame writes.
pub(crate) struct UpgradeResponse {
    response: Response,
    accept: String,
    offered_deflate: bool,
}

impl std::ops::Deref for UpgradeResponse {
    type Target = Response;

    fn deref(&self) -> &Self::Target {
        &self.response
    }
}

pub(crate) async fn upgrade(builder: reqwest::RequestBuilder) -> Result<UpgradeResponse, ()> {
    upgrade_with_extensions(builder, false).await
}

pub(crate) async fn upgrade_siwc(builder: reqwest::RequestBuilder) -> Result<UpgradeResponse, ()> {
    upgrade_with_extensions(builder, true).await
}

async fn upgrade_with_extensions(
    builder: reqwest::RequestBuilder,
    offered_deflate: bool,
) -> Result<UpgradeResponse, ()> {
    let (client, request) = builder.build_split();
    let mut request = request.map_err(|_| ())?;
    match request.url().scheme() {
        "ws" => request.url_mut().set_scheme("http").map_err(|_| ())?,
        "wss" => request.url_mut().set_scheme("https").map_err(|_| ())?,
        "http" | "https" => (),
        _ => return Err(()),
    }
    *request.version_mut() = Version::HTTP_11;
    let key = generate_key();
    let headers = request.headers_mut();
    headers.insert(CONNECTION, "Upgrade".parse().map_err(|_| ())?);
    headers.insert(UPGRADE, "websocket".parse().map_err(|_| ())?);
    headers.insert(SEC_WEBSOCKET_VERSION, "13".parse().map_err(|_| ())?);
    headers.insert(SEC_WEBSOCKET_KEY, key.parse().map_err(|_| ())?);
    if offered_deflate {
        headers.insert(SEC_WEBSOCKET_EXTENSIONS, deflate::offer());
    } else {
        headers.remove(SEC_WEBSOCKET_EXTENSIONS);
    }
    headers.remove(SEC_WEBSOCKET_PROTOCOL);
    Ok(UpgradeResponse {
        response: client.execute(request).await.map_err(|_| ())?,
        accept: derive_accept_key(key.as_bytes()),
        offered_deflate,
    })
}

impl UpgradeResponse {
    pub(crate) fn into_inner(self) -> Response {
        self.response
    }

    pub(crate) async fn into_websocket(self) -> Result<UpstreamWebSocket, ()> {
        let extension = validate_handshake(&self.response, &self.accept, self.offered_deflate)?;
        let upgraded = self.response.upgrade().await.map_err(|_| ())?;
        if let Some(extension) = extension {
            let socket = deflate::Socket::new(upgraded, extension)?;
            return Ok(UpstreamWebSocket {
                inner: SocketKind::Deflate(Box::new(socket)),
            });
        }
        let config = WebSocketConfig::default()
            .max_message_size(Some(super::MAX_WEBSOCKET_MESSAGE_BYTES))
            .max_frame_size(Some(super::MAX_WEBSOCKET_MESSAGE_BYTES));
        let socket = Socket::from_raw_socket(upgraded.compat(), Role::Client, Some(config)).await;
        Ok(UpstreamWebSocket {
            inner: SocketKind::Identity(Box::new(socket)),
        })
    }
}

fn validate_handshake(
    response: &Response,
    accept: &str,
    offered_deflate: bool,
) -> Result<Option<&'static str>, ()> {
    let headers = response.headers();
    let connection_upgrade = headers.get_all(CONNECTION).iter().any(|value| {
        value.to_str().is_ok_and(|tokens| {
            tokens
                .split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        })
    });
    if response.version() != Version::HTTP_11
        || response.status() != StatusCode::SWITCHING_PROTOCOLS
        || !connection_upgrade
        || headers.get_all(UPGRADE).iter().count() != 1
        || !headers
            .get(UPGRADE)
            .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"websocket"))
        || headers.get_all(SEC_WEBSOCKET_ACCEPT).iter().count() != 1
        || !headers
            .get(SEC_WEBSOCKET_ACCEPT)
            .is_some_and(|value| value.as_bytes() == accept.as_bytes())
        || headers.contains_key(SEC_WEBSOCKET_PROTOCOL)
    {
        return Err(());
    }
    deflate::accepted(headers, offered_deflate)
}

pub(crate) struct UpstreamWebSocket {
    inner: SocketKind,
}

enum SocketKind {
    Identity(Box<Socket>),
    Deflate(Box<deflate::Socket>),
}

pub(crate) struct SentPayload {
    pub(crate) body_bytes: u64,
    pub(crate) content_encoding: crate::usage::RequestContentEncoding,
}

impl UpstreamWebSocket {
    pub(crate) fn compression_candidate(&self, json_bytes: usize) -> bool {
        matches!(self.inner, SocketKind::Deflate(_)) && json_bytes >= deflate::COMPRESSION_THRESHOLD
    }

    pub(crate) async fn send_request(&mut self, payload: String) -> Result<SentPayload, ()> {
        match &mut self.inner {
            SocketKind::Identity(socket) => {
                let body_bytes = payload.len() as u64;
                super::io::send_request_message(socket, payload).await?;
                Ok(SentPayload {
                    body_bytes,
                    content_encoding: crate::usage::RequestContentEncoding::Identity,
                })
            }
            SocketKind::Deflate(socket) => socket.send_request(payload).await,
        }
    }
}

impl Stream for UpstreamWebSocket {
    type Item = Result<Message, ()>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match &mut self.inner {
            SocketKind::Identity(socket) => socket
                .poll_next_unpin(cx)
                .map(|item| item.map(|result| result.map_err(|_| ()).and_then(receive_message))),
            SocketKind::Deflate(socket) => socket.poll_next_unpin(cx),
        }
    }
}

impl Sink<Message> for UpstreamWebSocket {
    type Error = ();

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match &mut self.inner {
            SocketKind::Identity(socket) => Pin::new(socket).poll_ready(cx).map_err(|_| ()),
            SocketKind::Deflate(socket) => Pin::new(socket).poll_ready(cx),
        }
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        match &mut self.inner {
            SocketKind::Identity(socket) => Pin::new(socket)
                .start_send(send_message(item))
                .map_err(|_| ()),
            SocketKind::Deflate(socket) => Pin::new(socket).start_send(item),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match &mut self.inner {
            SocketKind::Identity(socket) => Pin::new(socket).poll_flush(cx).map_err(|_| ()),
            SocketKind::Deflate(socket) => Pin::new(socket).poll_flush(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        match &mut self.inner {
            SocketKind::Identity(socket) => Pin::new(socket).poll_close(cx).map_err(|_| ()),
            SocketKind::Deflate(socket) => Pin::new(socket).poll_close(cx),
        }
    }
}

fn receive_message(message: tungstenite::Message) -> Result<Message, ()> {
    Ok(match message {
        tungstenite::Message::Text(text) => Message::Text(text.to_string()),
        tungstenite::Message::Binary(bytes) => Message::Binary(bytes),
        tungstenite::Message::Ping(bytes) => Message::Ping(bytes),
        tungstenite::Message::Pong(bytes) => Message::Pong(bytes),
        tungstenite::Message::Close(frame) => Message::Close {
            code: frame
                .as_ref()
                .map_or_else(Default::default, |frame| u16::from(frame.code).into()),
            reason: frame.map_or_else(String::new, |frame| frame.reason.to_string()),
        },
        tungstenite::Message::Frame(_) => return Err(()),
    })
}

fn send_message(message: Message) -> tungstenite::Message {
    match message {
        Message::Text(text) => tungstenite::Message::Text(text.into()),
        Message::Binary(bytes) => tungstenite::Message::Binary(bytes),
        Message::Ping(bytes) => tungstenite::Message::Ping(bytes),
        Message::Pong(bytes) => tungstenite::Message::Pong(bytes),
        Message::Close { code, reason } => {
            tungstenite::Message::Close(Some(tungstenite::protocol::CloseFrame {
                code: u16::from(code).into(),
                reason: reason.into(),
            }))
        }
    }
}

#[cfg(test)]
mod tests;

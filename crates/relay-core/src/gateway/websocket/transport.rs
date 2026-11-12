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

type Socket = async_tungstenite::WebSocketStream<tokio_util::compat::Compat<reqwest::Upgraded>>;

/// Retains Relay's reqwest client, proxy and TLS policy while exposing frame writes.
pub(crate) struct UpgradeResponse {
    response: Response,
    accept: String,
}

impl std::ops::Deref for UpgradeResponse {
    type Target = Response;

    fn deref(&self) -> &Self::Target {
        &self.response
    }
}

pub(crate) async fn upgrade(builder: reqwest::RequestBuilder) -> Result<UpgradeResponse, ()> {
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
    // Relay negotiates neither extensions nor subprotocols on this transport.
    headers.remove(SEC_WEBSOCKET_EXTENSIONS);
    headers.remove(SEC_WEBSOCKET_PROTOCOL);
    Ok(UpgradeResponse {
        response: client.execute(request).await.map_err(|_| ())?,
        accept: derive_accept_key(key.as_bytes()),
    })
}

impl UpgradeResponse {
    pub(crate) fn into_inner(self) -> Response {
        self.response
    }

    pub(crate) async fn into_websocket(self) -> Result<UpstreamWebSocket, ()> {
        validate_handshake(&self.response, &self.accept)?;
        let config = WebSocketConfig::default()
            .max_message_size(Some(super::MAX_WEBSOCKET_MESSAGE_BYTES))
            .max_frame_size(Some(super::MAX_WEBSOCKET_MESSAGE_BYTES));
        let inner = Socket::from_raw_socket(
            self.response.upgrade().await.map_err(|_| ())?.compat(),
            Role::Client,
            Some(config),
        )
        .await;
        Ok(UpstreamWebSocket { inner })
    }
}

fn validate_handshake(response: &Response, accept: &str) -> Result<(), ()> {
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
        || headers.contains_key(SEC_WEBSOCKET_EXTENSIONS)
    {
        return Err(());
    }
    Ok(())
}

pub(crate) struct UpstreamWebSocket {
    inner: Socket,
}

impl UpstreamWebSocket {
    pub(crate) async fn send_request(&mut self, payload: String) -> Result<(), ()> {
        super::io::send_request_message(&mut self.inner, payload).await
    }
}

impl Stream for UpstreamWebSocket {
    type Item = Result<Message, ()>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.poll_next_unpin(cx).map(|item| {
            item.map(|result| {
                result
                    .map_err(|_| ())
                    .and_then(|message| message.try_into().map_err(|_| ()))
            })
        })
    }
}

impl Sink<Message> for UpstreamWebSocket {
    type Error = tungstenite::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner).poll_ready(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        Pin::new(&mut self.inner).start_send(item.into())
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.inner).poll_close(cx)
    }
}

#[cfg(test)]
mod tests;

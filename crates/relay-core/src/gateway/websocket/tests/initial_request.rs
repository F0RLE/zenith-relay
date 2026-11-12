use super::*;
use crate::gateway::websocket::{
    INITIAL_MESSAGE_TIMEOUT, WEBSOCKET_HEARTBEAT_INTERVAL, client_socket::WebSocket,
    read_initial_request,
};
use axum::{Router, extract::ws::WebSocketUpgrade, routing::get};
use futures_util::{SinkExt, StreamExt};

async fn socket_pair() -> (
    WebSocket,
    crate::gateway::websocket::transport::UpstreamWebSocket,
    tokio::task::JoinHandle<()>,
) {
    let (accepted, mut sockets) = tokio::sync::mpsc::channel(1);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new().route(
        "/ws",
        get(move |upgrade: WebSocketUpgrade| {
            let accepted = accepted.clone();
            async move {
                upgrade.on_upgrade(move |socket| async move {
                    accepted.send(socket).await.unwrap();
                })
            }
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let response = crate::gateway::websocket::transport::upgrade(
        reqwest::Client::new().get(format!("ws://{address}/ws")),
    )
    .await
    .unwrap();
    let client = response.into_websocket().await.unwrap();
    let socket = WebSocket::new(sockets.recv().await.unwrap());
    (socket, client, server)
}

#[tokio::test]
async fn initial_request_heartbeat_starts_without_dispatch_and_keeps_its_deadline() {
    let (mut downstream, mut client, server) = socket_pair().await;
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    tokio::time::pause();
    let started = tokio::time::Instant::now();
    let read = async move {
        read_initial_request(&mut downstream, &runtime, &key, &HeaderMap::new()).await
    };
    tokio::pin!(read);
    let first = tokio::select! {
        result = &mut read => panic!("initial request ended before heartbeat: {}", result.err().unwrap().0.message),
        message = client.next() => message.unwrap().unwrap(),
    };
    assert!(matches!(first, reqwest_websocket::Message::Ping(_)));
    assert!(started.elapsed() < WEBSOCKET_HEARTBEAT_INTERVAL);
    client
        .send(reqwest_websocket::Message::Pong(Default::default()))
        .await
        .unwrap();
    tokio::time::advance(INITIAL_MESSAGE_TIMEOUT.saturating_sub(started.elapsed())).await;
    let failure = read.await.err().unwrap().0;
    assert_eq!(failure.message, GatewayFailure::request_timeout().message);
    // Tokio's timer wheel rounds the fixed deadline to millisecond resolution.
    assert!(started.elapsed() >= INITIAL_MESSAGE_TIMEOUT);
    assert!(started.elapsed() <= INITIAL_MESSAGE_TIMEOUT + std::time::Duration::from_millis(1));
    server.abort();
}

#[tokio::test]
async fn initial_request_close_stops_the_heartbeat_loop() {
    let (mut downstream, mut client, server) = socket_pair().await;
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    client
        .send(reqwest_websocket::Message::Close {
            code: reqwest_websocket::CloseCode::Normal,
            reason: String::new(),
        })
        .await
        .unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        read_initial_request(&mut downstream, &runtime, &key, &HeaderMap::new()),
    )
    .await
    .unwrap();
    assert_eq!(
        result.err().unwrap().0.message,
        GatewayFailure::client_closed().message
    );
    server.abort();
}

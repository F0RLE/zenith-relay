use super::*;
use futures_util::SinkExt;

#[test]
fn frame_conversion_preserves_payloads_and_private_close_codes() {
    use tungstenite::Message as Frame;
    for message in [
        Frame::Text("synthetic 😀 message".into()),
        Frame::Binary(vec![0, 255, 128].into()),
        Frame::Ping(vec![0, 1].into()),
        Frame::Pong(vec![2, 3].into()),
        Frame::Close(Some(tungstenite::protocol::CloseFrame {
            code: 4001.into(),
            reason: "synthetic close 😀".into(),
        })),
    ] {
        assert_eq!(
            send_message(receive_message(message.clone()).unwrap()),
            message
        );
    }
    assert!(matches!(
        receive_message(tungstenite::Message::Close(None)).unwrap(),
        Message::Close {
            code,
            reason,
        } if code == reqwest_websocket::CloseCode::default() && reason.is_empty()
    ));
    assert!(
        receive_message(tungstenite::Message::Frame(
            tungstenite::protocol::frame::Frame::ping(Vec::new())
        ))
        .is_err()
    );
}

fn response() -> axum::http::response::Builder {
    axum::http::Response::builder()
        .version(Version::HTTP_11)
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .header(CONNECTION, "keep-alive, Upgrade")
        .header(UPGRADE, "websocket")
        .header(SEC_WEBSOCKET_ACCEPT, "expected")
}

#[test]
fn handshake_requires_exact_accept_and_no_unrequested_features() {
    let valid = response().body("").unwrap().into();
    assert_eq!(validate_handshake(&valid, "expected"), Ok(()));
    for invalid in [
        response().status(StatusCode::OK),
        response().version(Version::HTTP_2),
        response().header(SEC_WEBSOCKET_ACCEPT, "other"),
        response().header(SEC_WEBSOCKET_PROTOCOL, "unrequested"),
        response().header(SEC_WEBSOCKET_EXTENSIONS, "permessage-deflate"),
        axum::http::Response::builder().status(StatusCode::SWITCHING_PROTOCOLS),
    ] {
        assert_eq!(
            validate_handshake(&invalid.body("").unwrap().into(), "expected"),
            Err(())
        );
    }
    assert_eq!(validate_handshake(&valid, "other"), Err(()));
}

#[tokio::test]
async fn fragmented_utf8_roundtrips_as_one_message_with_control_frames() {
    use axum::{Router, extract::ws::WebSocketUpgrade, routing::get};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new().route(
        "/ws",
        get(|upgrade: WebSocketUpgrade| async {
            upgrade.on_upgrade(|mut socket| async move {
                let message = socket.recv().await.unwrap().unwrap();
                assert!(matches!(&message, axum::extract::ws::Message::Text(_)));
                socket.send(message).await.unwrap();
                if let Some(Ok(axum::extract::ws::Message::Ping(bytes))) = socket.recv().await {
                    socket
                        .send(axum::extract::ws::Message::Pong(bytes))
                        .await
                        .unwrap();
                }
            })
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let exchange = async {
        let response = upgrade(reqwest::Client::new().get(format!("ws://{address}/ws")))
            .await
            .unwrap();
        let mut socket = response.into_websocket().await.unwrap();
        let payload = format!(
            "{}😀suffix",
            "a".repeat(super::super::io::MAX_OUTBOUND_FRAME_BYTES - 1)
        );
        socket.send_request(payload.clone()).await.unwrap();
        assert!(
            matches!(socket.next().await.unwrap().unwrap(), Message::Text(text) if text == payload)
        );
        let ping = reqwest_websocket::Bytes::from_static(b"synthetic-ping");
        socket.send(Message::Ping(ping.clone())).await.unwrap();
        assert!(
            matches!(socket.next().await.unwrap().unwrap(), Message::Pong(bytes) if bytes == ping)
        );
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), exchange).await;
    server.abort();
    result.unwrap();
}

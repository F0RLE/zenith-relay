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
    assert_eq!(validate_handshake(&valid, "expected", false), Ok(None));
    assert_eq!(validate_handshake(&valid, "expected", true), Ok(None));
    for invalid in [
        response().status(StatusCode::OK),
        response().version(Version::HTTP_2),
        response().header(SEC_WEBSOCKET_ACCEPT, "other"),
        response().header(SEC_WEBSOCKET_PROTOCOL, "unrequested"),
        response().header(SEC_WEBSOCKET_EXTENSIONS, "permessage-deflate"),
        axum::http::Response::builder().status(StatusCode::SWITCHING_PROTOCOLS),
    ] {
        assert_eq!(
            validate_handshake(&invalid.body("").unwrap().into(), "expected", false),
            Err(())
        );
    }
    assert_eq!(validate_handshake(&valid, "other", true), Err(()));
}

#[test]
fn siwc_handshake_accepts_only_agreed_deflate_parameters() {
    for (extension, reset_server) in [
        ("permessage-deflate", false),
        ("permessage-deflate; server_no_context_takeover", true),
        (
            "permessage-deflate; client_no_context_takeover; server_no_context_takeover",
            true,
        ),
        ("permessage-deflate; server_max_window_bits=\"12\"", false),
    ] {
        let accepted = response()
            .header(SEC_WEBSOCKET_EXTENSIONS, extension)
            .body("")
            .unwrap()
            .into();
        let codec = validate_handshake(&accepted, "expected", true)
            .unwrap()
            .unwrap();
        assert!(codec.contains("client_no_context_takeover"));
        assert_eq!(codec.contains("server_no_context_takeover"), reset_server);
        assert_eq!(validate_handshake(&accepted, "expected", false), Err(()));
    }
    for extension in [
        "permessage-deflate, unknown-extension",
        "permessage-deflate; client_no_context_takeover; client_no_context_takeover",
        "permessage-deflate; server_no_context_takeover=1",
        "permessage-deflate; client_max_window_bits=12",
        "permessage-deflate; server_max_window_bits=07",
        "permessage-deflate; server_max_window_bits=12; server_max_window_bits=10",
    ] {
        let invalid = response()
            .header(SEC_WEBSOCKET_EXTENSIONS, extension)
            .body("")
            .unwrap()
            .into();
        assert_eq!(validate_handshake(&invalid, "expected", true), Err(()));
    }
    let duplicate = response()
        .header(SEC_WEBSOCKET_EXTENSIONS, "permessage-deflate")
        .header(SEC_WEBSOCKET_EXTENSIONS, "permessage-deflate")
        .body("")
        .unwrap()
        .into();
    assert_eq!(validate_handshake(&duplicate, "expected", true), Err(()));
}

#[tokio::test]
async fn declined_deflate_keeps_fragmented_utf8_and_control_frames() {
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
        let response = upgrade_siwc(reqwest::Client::new().get(format!("ws://{address}/ws")))
            .await
            .unwrap();
        let mut socket = response.into_websocket().await.unwrap();
        let payload = format!(
            "{}😀suffix",
            "a".repeat(super::super::io::MAX_OUTBOUND_FRAME_BYTES - 1)
        );
        let sent = socket.send_request(payload.clone()).await.unwrap();
        assert_eq!(sent.body_bytes, payload.len() as u64);
        assert_eq!(
            sent.content_encoding,
            crate::usage::RequestContentEncoding::Identity
        );
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

#[tokio::test]
async fn siwc_negotiates_deflate_for_large_frames_but_not_small_deltas() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use yawc::{
        Options, Role, WebSocket,
        frame::{Frame, OpCode},
    };

    let text = "synthetic data ".repeat(57_000);
    let body = format!(
        r#"{{"input":["{}"]}}"#,
        &text[..852_502 - r#"{"input":[""]}"#.len()]
    );
    assert_eq!(body.len(), 852_502);
    let expected = body.clone();
    let small = r#"{"input":["synthetic follow-up"]}"#;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move {
        let (mut io, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            assert!(request.len() < 4096);
            request.push(io.read_u8().await.unwrap());
        }
        let request = String::from_utf8(request).unwrap();
        let header = |name: &str| {
            request
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.trim())
                .unwrap()
        };
        assert_eq!(
            header("sec-websocket-extensions"),
            "permessage-deflate; client_no_context_takeover"
        );
        // A bare acceptance keeps the server's dictionary across messages.
        let agreed = "permessage-deflate";
        let accept = derive_accept_key(header("sec-websocket-key").as_bytes());
        io.write_all(
            format!("HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Extensions: {agreed}\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
        let mut socket = WebSocket::from_stream_with_extensions(
            io,
            Role::Server,
            Some(agreed),
            Options::default()
                .with_low_latency_compression()
                .with_compression_threshold(0)
                .with_utf8(),
        )
        .unwrap();
        let frame = socket.next_frame().await.unwrap();
        assert_eq!(frame.opcode(), OpCode::Text);
        assert_eq!(frame.as_str(), expected);
        socket.send(Frame::ping("synthetic")).await.unwrap();
        socket
            .send(Frame::text("synthetic 😀 reply"))
            .await
            .unwrap();
        let pong = socket.next_frame().await.unwrap();
        assert_eq!(pong.opcode(), OpCode::Pong);
        assert_eq!(pong.as_str(), "synthetic");
        assert_eq!(socket.next_frame().await.unwrap().as_str(), small);
        socket
            .send(Frame::text("synthetic 😀 reply"))
            .await
            .unwrap();
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut socket = upgrade_siwc(reqwest::Client::new().get(format!("ws://{address}/ws")))
            .await
            .unwrap()
            .into_websocket()
            .await
            .unwrap();
        let bytes = body.len() as u64;
        let sent = socket.send_request(body).await.unwrap();
        use crate::usage::RequestContentEncoding::{Identity, PerMessageDeflate};
        assert_eq!(sent.content_encoding, PerMessageDeflate);
        assert!((1..bytes / 4).contains(&sent.body_bytes));
        assert!(matches!(socket.next().await.unwrap().unwrap(), Message::Ping(_)));
        socket.flush().await.unwrap();
        assert!(matches!(socket.next().await.unwrap().unwrap(), Message::Text(text) if text == "synthetic 😀 reply"));
        let sent = socket.send_request(small.into()).await.unwrap();
        assert_eq!(sent.content_encoding, Identity);
        assert_eq!(sent.body_bytes, small.len() as u64);
        assert!(matches!(socket.next().await.unwrap().unwrap(), Message::Text(text) if text == "synthetic 😀 reply"));
        peer.await.unwrap();
    })
    .await
    .unwrap();
}

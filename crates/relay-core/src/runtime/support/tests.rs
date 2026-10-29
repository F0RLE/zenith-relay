use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn runtime_http2_does_not_hide_refused_stream_retries() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/responses", listener.local_addr().unwrap());
    let requests = Arc::new(AtomicUsize::new(0));
    let observed = requests.clone();
    let server = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let observed = observed.clone();
            connections.spawn(async move {
                let mut preface = [0; 24];
                socket.read_exact(&mut preface).await.unwrap();
                assert_eq!(&preface, b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
                socket
                    .write_all(&[0, 0, 0, 4, 0, 0, 0, 0, 0])
                    .await
                    .unwrap();
                loop {
                    let mut header = [0_u8; 9];
                    if socket.read_exact(&mut header).await.is_err() {
                        break;
                    }
                    let length = u32::from_be_bytes([0, header[0], header[1], header[2]]);
                    assert!(length <= 16 * 1024);
                    let mut payload = vec![0; length as usize];
                    if socket.read_exact(&mut payload).await.is_err() {
                        break;
                    }
                    if header[3] == 4 && header[4] & 1 == 0 {
                        if socket
                            .write_all(&[0, 0, 0, 4, 1, 0, 0, 0, 0])
                            .await
                            .is_err()
                        {
                            break;
                        }
                    } else if header[3] == 1 {
                        observed.fetch_add(1, Ordering::SeqCst);
                        // RST_STREAM / REFUSED_STREAM: safe at the protocol layer,
                        // but a resend still belongs to Relay's attempt budget.
                        let mut reset = vec![0, 0, 4, 3, 0];
                        reset.extend_from_slice(&header[5..9]);
                        reset.extend_from_slice(&7_u32.to_be_bytes());
                        if socket.write_all(&reset).await.is_err() {
                            break;
                        }
                    }
                }
            });
        }
    });
    for (builder, expected) in [
        (reqwest::Client::builder(), 3),
        (runtime_client_builder(None), 1),
    ] {
        let before = requests.load(Ordering::SeqCst);
        let client = builder.no_proxy().http2_prior_knowledge().build().unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            client.post(&url).body("synthetic request").send(),
        )
        .await
        .expect("the local HTTP/2 rejection must finish");
        assert!(result.is_err());
        assert_eq!(requests.load(Ordering::SeqCst) - before, expected);
    }
    server.abort();
}

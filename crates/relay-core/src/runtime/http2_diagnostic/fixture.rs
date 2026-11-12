use axum::{body::Bytes, http::Response};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::{net::TcpListener, task::JoinSet};

#[derive(Clone, Copy)]
pub(super) enum Reply {
    Refused,
    Cancelled,
    RateLimitThenSuccess,
}

pub(super) struct Fixture {
    pub(super) url: String,
    pub(super) requests: Arc<AtomicUsize>,
}

impl Fixture {
    pub(super) async fn start(tasks: &mut JoinSet<()>, reply: Reply) -> Result<Self, &'static str> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| "http2_fixture_bind_failed")?;
        let url = format!(
            "http://{}/v1",
            listener
                .local_addr()
                .map_err(|_| "http2_fixture_address_failed")?
        );
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        tasks.spawn(async move {
            let mut connections = JoinSet::new();
            for _ in 0..8 {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                while connections.try_join_next().is_some() {}
                if connections.len() < 4 {
                    let observed = observed.clone();
                    connections.spawn(async move {
                        let _ = tokio::time::timeout(
                            Duration::from_secs(5),
                            serve(socket, observed, reply),
                        )
                        .await;
                    });
                }
            }
        });
        Ok(Self { url, requests })
    }
}

pub(super) async fn serve<Io>(socket: Io, requests: Arc<AtomicUsize>, reply: Reply)
where
    Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut builder = h2::server::Builder::new();
    builder.max_header_list_size(4096).max_concurrent_streams(4);
    let Ok(mut connection) = builder.handshake::<_, Bytes>(socket).await else {
        return;
    };
    let mut responses = JoinSet::new();
    for _ in 0..8 {
        let Some(Ok((request, mut respond))) = connection.accept().await else {
            break;
        };
        let index = requests.fetch_add(1, Ordering::SeqCst);
        match reply {
            Reply::Refused => respond.send_reset(h2::Reason::REFUSED_STREAM),
            Reply::Cancelled => respond.send_reset(h2::Reason::CANCEL),
            Reply::RateLimitThenSuccess => {
                while responses.try_join_next().is_some() {}
                if responses.len() >= 4 {
                    return;
                }
                responses.spawn(async move {
                    let mut body = request.into_body();
                    let mut received = 0;
                    while let Some(chunk) = body.data().await {
                        let Ok(chunk) = chunk else { return };
                        received += chunk.len();
                        if received > 4096 { respond.send_reset(h2::Reason::CANCEL); return; }
                        if body.flow_control().release_capacity(chunk.len()).is_err() { return; }
                    }
                    let (status, payload): (u16, &'static [u8]) = if index == 0 {
                        (429, br#"{"error":{"code":"rate_limit_exceeded","message":"Synthetic limit"}}"#)
                    } else {
                        (200, br#"{"id":"synthetic-response","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}}"#)
                    };
                    let Ok(response) = Response::builder().status(status).header("content-type", "application/json").body(()) else { return };
                    if let Ok(mut stream) = respond.send_response(response, false) {
                        let _ = stream.send_data(Bytes::from_static(payload), true);
                    }
                });
            }
        }
    }
}

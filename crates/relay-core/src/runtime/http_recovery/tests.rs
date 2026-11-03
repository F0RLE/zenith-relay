use super::*;
use bytes::Bytes;
use futures_util::{StreamExt, stream};

fn select(recovery: &HttpRecovery, candidate: &str, origin: &str) -> HttpPoolLease {
    recovery
        .select(
            candidate,
            &Url::parse(origin).unwrap(),
            &Client::new(),
            None,
        )
        .unwrap()
}

#[test]
fn independent_failed_generations_trigger_fallback_but_multiplexed_failures_do_not() {
    let recovery = HttpRecovery::default();
    let first = select(
        &recovery,
        "account-a",
        "https://synthetic.invalid/v1/responses",
    );
    let peer = first.clone();
    let now = Instant::now();
    first.retire(now);
    peer.retire(now);
    let second = select(
        &recovery,
        "account-a",
        "https://synthetic.invalid/v1/responses",
    );
    assert!(!Arc::ptr_eq(&first.pool, &second.pool));
    assert!(!second.http1_only());
    second.retire(now);
    let fallback = select(
        &recovery,
        "account-a",
        "https://synthetic.invalid/v1/responses",
    );
    assert!(fallback.http1_only());
    first.retire(now);
    second.retire(now);
    assert!(Arc::ptr_eq(
        &fallback.pool,
        &select(
            &recovery,
            "account-a",
            "https://synthetic.invalid/v1/responses"
        )
        .pool
    ));
}

#[test]
fn recovery_is_scoped_to_the_candidate_and_origin_not_the_url_path() {
    let recovery = HttpRecovery::default();
    let first = select(&recovery, "a", "https://synthetic.invalid/v1/responses");
    let other_account = select(&recovery, "b", "https://synthetic.invalid/v1/responses");
    let other_origin = select(&recovery, "a", "https://other.invalid/v1/responses");
    assert!(Arc::ptr_eq(
        &first.pool,
        &select(
            &recovery,
            "a",
            "https://synthetic.invalid/v1/chat/completions"
        )
        .pool
    ));
    first.retire(Instant::now());
    select(&recovery, "a", "https://synthetic.invalid/v1/responses").retire(Instant::now());
    assert!(Arc::ptr_eq(
        &other_account.pool,
        &select(&recovery, "b", "https://synthetic.invalid/v1/responses").pool
    ));
    assert!(Arc::ptr_eq(
        &other_origin.pool,
        &select(&recovery, "a", "https://other.invalid/v1/responses").pool
    ));
}

#[test]
fn fallback_expires_without_reusing_a_retired_pool_or_extending_on_each_failure() {
    let recovery = HttpRecovery::default();
    let url = Url::parse("https://synthetic.invalid/v1/responses").unwrap();
    let client = Client::new();
    let now = Instant::now();
    let select_at = |time| recovery.select_at("a", &url, &client, None, time).unwrap();
    select_at(now).retire(now);
    select_at(now).retire(now);
    let fallback = select_at(now);
    assert!(fallback.http1_only());
    fallback.retire(now + Duration::from_secs(60));
    let resumed = select_at(now + HTTP1_COOLDOWN);
    assert!(!resumed.http1_only());
    assert!(!Arc::ptr_eq(&fallback.pool, &resumed.pool));
    fallback.retire(now + HTTP1_COOLDOWN);
    assert!(Arc::ptr_eq(
        &resumed.pool,
        &select_at(now + HTTP1_COOLDOWN).pool
    ));
}

#[test]
fn old_or_successfully_recovered_failures_do_not_accumulate() {
    let recovery = HttpRecovery::default();
    let now = Instant::now();
    let first = select(&recovery, "a", "https://synthetic.invalid/");
    first.retire(now);
    let second = select(&recovery, "a", "https://synthetic.invalid/");
    second.completed();
    second.retire(now);
    assert!(!select(&recovery, "a", "https://synthetic.invalid/").http1_only());
    select(&recovery, "a", "https://synthetic.invalid/").retire(now + FAILURE_WINDOW);
    assert!(!select(&recovery, "a", "https://synthetic.invalid/").http1_only());
}

#[test]
fn observations_are_bounded_and_inactive_routes_expire() {
    let recovery = HttpRecovery::default();
    let client = Client::new();
    let url = Url::parse("https://synthetic.invalid/").unwrap();
    let now = Instant::now();
    let first = recovery.select_at("0", &url, &client, None, now).unwrap();
    let original = Arc::downgrade(&first.pool);
    drop(first);
    for index in 1..MAX_ROUTES {
        assert!(
            recovery
                .select_at(&index.to_string(), &url, &client, None, now)
                .is_some()
        );
    }
    assert!(
        recovery
            .select_at("overflow", &url, &client, None, now)
            .is_none()
    );
    let replacement = recovery
        .select_at("0", &url, &client, None, now + IDLE_TTL)
        .unwrap();
    assert!(original.upgrade().is_none());
    assert!(Arc::ptr_eq(
        &replacement.pool,
        &recovery
            .select_at("0", &url, &client, None, now + IDLE_TTL)
            .unwrap()
            .pool
    ));
    assert_eq!(crate::poison::mutex(&recovery.observations).len(), 1);
}

#[test]
fn a_quiet_active_request_keeps_its_recovery_owner_past_the_idle_ttl() {
    let recovery = HttpRecovery::default();
    let client = Client::new();
    let url = Url::parse("https://synthetic.invalid/").unwrap();
    let now = Instant::now();
    let active = recovery.select_at("a", &url, &client, None, now).unwrap();
    let later = now + IDLE_TTL + Duration::from_secs(1);
    let peer = recovery.select_at("a", &url, &client, None, later).unwrap();
    assert!(Arc::ptr_eq(&active.pool, &peer.pool));
    active.retire(later);
    let replacement = recovery.select_at("a", &url, &client, None, later).unwrap();
    assert!(!Arc::ptr_eq(&active.pool, &replacement.pool));
    peer.retire(later);
    assert!(Arc::ptr_eq(
        &replacement.pool,
        &recovery
            .select_at("a", &url, &client, None, later)
            .unwrap()
            .pool
    ));
}

#[test]
fn stream_resets_timeouts_and_validation_errors_do_not_retire_the_connection() {
    let stream_reset = h2::Error::from(h2::Reason::CANCEL);
    assert!(!connection_failure(&stream_reset, true));
    for kind in [
        io::ErrorKind::TimedOut,
        io::ErrorKind::InvalidData,
        io::ErrorKind::ConnectionRefused,
    ] {
        assert!(!connection_failure(
            &io::Error::new(kind, "synthetic"),
            true
        ));
    }
    let reset = io::Error::new(io::ErrorKind::ConnectionReset, "synthetic");
    assert!(!connection_failure(&reset, false));
    assert!(connection_failure(&reset, true));
}

#[tokio::test]
async fn broken_http2_handshake_retires_the_pool_without_retrying_the_request() {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!(
        "http://{}/v1/responses",
        listener.local_addr().unwrap()
    ))
    .unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut preface = [0_u8; 24];
        socket.read_exact(&mut preface).await.unwrap();
        assert_eq!(&preface, b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
        drop(socket);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let client = super::super::support::runtime_client_builder(None)
        .no_proxy()
        .http2_prior_knowledge()
        .build()
        .unwrap();
    let recovery = HttpRecovery::default();
    let first = recovery.select("a", &url, &client, None).unwrap();
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        first.client().post(url.clone()).body("synthetic").send(),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(connection_failure(&error, false), "{error:?}");
    first.observe_error(&error);
    let next = recovery.select("a", &url, &client, None).unwrap();
    assert!(!Arc::ptr_eq(&first.pool, &next.pool));
    assert!(!next.http1_only());
    server.await.unwrap();
}

#[tokio::test]
async fn replacement_and_http1_fallback_keep_the_configured_proxy() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = ProxyConfig::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                request.push(socket.read_u8().await.unwrap());
            }
            assert!(request.starts_with(b"GET http://synthetic.invalid/v1/responses HTTP/1.1\r\n"));
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
        }
    });
    let recovery = HttpRecovery::default();
    let url = Url::parse("http://synthetic.invalid/v1/responses").unwrap();
    let client = fresh_client(Some(&proxy), false).unwrap();
    for http1_expected in [false, true] {
        recovery
            .select("a", &url, &client, Some(&proxy))
            .unwrap()
            .retire(Instant::now());
        let lease = recovery.select("a", &url, &client, Some(&proxy)).unwrap();
        assert_eq!(lease.http1_only(), http1_expected);
        let response = tokio::time::timeout(
            Duration::from_secs(2),
            lease.client().get(url.clone()).send(),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(response.text().await.unwrap(), "ok");
    }
    server.await.unwrap();
}

#[tokio::test]
async fn response_observation_preserves_body_url_headers_extensions_and_known_status() {
    let recovery = HttpRecovery::default();
    let lease = select(&recovery, "a", "https://synthetic.invalid/v1/responses");
    let response = axum::http::Response::builder()
        .status(429)
        .version(Version::HTTP_2)
        .url(Url::parse("https://synthetic.invalid/v1/responses").unwrap())
        .header("retry-after", "7")
        .extension(37_u64)
        .body("synthetic body")
        .unwrap();
    let response = lease.clone().observe_response(response.into());
    assert_eq!(
        response.url().as_str(),
        "https://synthetic.invalid/v1/responses"
    );
    assert_eq!(response.status().as_u16(), 429);
    assert_eq!(response.headers()["retry-after"], "7");
    assert_eq!(response.extensions().get::<u64>(), Some(&37));
    assert_eq!(response.bytes().await.unwrap(), "synthetic body");
    assert!(Arc::ptr_eq(
        &lease.pool,
        &select(&recovery, "a", "https://synthetic.invalid/v1/responses").pool
    ));
}

#[tokio::test]
async fn midstream_connection_error_retires_only_future_requests_without_replaying_output() {
    let recovery = HttpRecovery::default();
    let lease = select(&recovery, "a", "https://synthetic.invalid/");
    let original = lease.pool.clone();
    let body = reqwest::Body::wrap_stream(stream::iter([
        Ok(Bytes::from_static(b"synthetic tool output")),
        Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "synthetic disconnect",
        )),
    ]));
    let response = axum::http::Response::builder()
        .version(Version::HTTP_2)
        .body(body)
        .unwrap();
    let mut output = lease.observe_response(response.into()).bytes_stream();
    assert_eq!(
        output.next().await.unwrap().unwrap(),
        "synthetic tool output"
    );
    assert!(output.next().await.unwrap().is_err());
    assert!(output.next().await.is_none());
    let next = select(&recovery, "a", "https://synthetic.invalid/");
    assert!(!Arc::ptr_eq(&original, &next.pool));
    assert!(!next.http1_only());
}

#[tokio::test]
async fn observing_http2_preserves_data_frames_and_trailers() {
    struct Frames(std::collections::VecDeque<hyper::body::Frame<Bytes>>);
    impl HttpBody for Frames {
        type Data = Bytes;
        type Error = std::convert::Infallible;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
        ) -> Poll<Option<std::result::Result<hyper::body::Frame<Bytes>, Self::Error>>> {
            Poll::Ready(self.0.pop_front().map(Ok))
        }
    }
    let recovery = HttpRecovery::default();
    let lease = select(&recovery, "a", "https://synthetic.invalid/");
    let mut trailers = axum::http::HeaderMap::new();
    trailers.insert("x-synthetic", "retained".parse().unwrap());
    let frames = Frames(std::collections::VecDeque::from([
        hyper::body::Frame::data(Bytes::from_static(b"synthetic")),
        hyper::body::Frame::trailers(trailers.clone()),
    ]));
    let response = axum::http::Response::builder()
        .version(Version::HTTP_2)
        .body(reqwest::Body::wrap(frames))
        .unwrap();
    let observed = lease.observe_response(response.into());
    let response: axum::http::Response<reqwest::Body> = observed.into();
    let mut body = response.into_body();
    let data = futures_util::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(data.into_data().unwrap(), "synthetic");
    let frame = futures_util::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(frame.into_trailers().unwrap(), trailers);
}

#[tokio::test]
async fn malformed_provider_output_does_not_trigger_connection_recovery() {
    let recovery = HttpRecovery::default();
    let lease = select(&recovery, "a", "https://synthetic.invalid/");
    let body = reqwest::Body::wrap_stream(stream::iter([Err::<Bytes, _>(io::Error::new(
        io::ErrorKind::InvalidData,
        "synthetic malformed data",
    ))]));
    let response = axum::http::Response::builder()
        .version(Version::HTTP_2)
        .body(body)
        .unwrap();
    assert!(
        lease
            .clone()
            .observe_response(response.into())
            .bytes()
            .await
            .is_err()
    );
    assert!(Arc::ptr_eq(
        &lease.pool,
        &select(&recovery, "a", "https://synthetic.invalid/").pool
    ));
}

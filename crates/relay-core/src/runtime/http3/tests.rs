use super::*;
use axum::http::{HeaderMap, Method, Response};
use bytes::{Buf, Bytes};
use std::sync::atomic::{AtomicUsize, Ordering};

fn request(url: &str) -> Request {
    let mut request = Request::new(Method::POST, Url::parse(url).unwrap());
    request
        .headers_mut()
        .insert("authorization", "Bearer synthetic".parse().unwrap());
    request
        .headers_mut()
        .insert("cookie", "synthetic=value".parse().unwrap());
    *request.body_mut() = Some("synthetic body".into());
    request
}

#[test]
fn http3_never_bypasses_explicit_or_system_proxies_or_http1_only_requests() {
    let transport = Http3Transport::with_enabled(true);
    let client = test_client();
    let mut https = request("https://provider.test/v1/responses");
    assert!(
        transport
            .select_client(&mut https, false, "route", &client)
            .is_none()
    );
    let mut http = request("http://127.0.0.1/v1/responses");
    assert!(
        transport
            .select_client(&mut http, true, "route", &client)
            .is_none()
    );
    let mut upgrade = request("https://provider.test/v1/responses");
    upgrade
        .headers_mut()
        .insert("upgrade", "websocket".parse().unwrap());
    assert!(
        transport
            .select_client(&mut upgrade, true, "route", &client)
            .is_none()
    );
    let mut metadata = request("https://provider.test/models");
    *metadata.method_mut() = Method::GET;
    assert!(
        transport
            .select_client(&mut metadata, true, "route", &client)
            .is_none()
    );
    assert!(crate::poison::mutex(&transport.origins).is_empty());

    let mut streaming = request("https://provider.test/v1/responses");
    *streaming.body_mut() = Some(reqwest::Body::wrap_stream(futures_util::stream::empty::<
        Result<Bytes, std::io::Error>,
    >()));
    assert!(
        transport
            .select_client(&mut streaming, true, "route", &client)
            .is_none()
    );

    let mut proxied = Http3Transport::with_enabled(true);
    proxied.proxy = hyper_util::client::proxy::matcher::Matcher::builder()
        .https("http://127.0.0.1:8080")
        .build();
    assert!(
        proxied
            .select_client(&mut https, true, "route", &client)
            .is_none()
    );
    assert!(crate::poison::mutex(&proxied.origins).is_empty());
    let disabled = Http3Transport::with_enabled(false);
    assert!(
        disabled
            .select_client(&mut https, true, "route", &client)
            .is_none()
    );
    assert!(crate::poison::mutex(&disabled.origins).is_empty());
}

#[tokio::test]
async fn http3_send_start_deadline_bounds_the_unbounded_reqwest_handshake() {
    let socket = tokio::net::UdpSocket::bind("[::1]:0").await.unwrap();
    let url = format!(
        "https://[::1]:{}/v1/responses",
        socket.local_addr().unwrap().port()
    );
    let client = super::super::support::runtime_client_builder(None)
        .no_proxy()
        .connect_timeout(Duration::from_millis(25))
        .http3_max_idle_timeout(Duration::ZERO)
        .build()
        .unwrap();
    let make_request = || {
        let mut request = request(&url);
        *request.version_mut() = Version::HTTP_3;
        request
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(150), client.execute(make_request()))
            .await
            .is_err(),
        "reqwest's H3 handshake bypasses the configured 25ms connect timeout"
    );
    let result =
        super::send::execute_with_start_timeout(&client, make_request(), Duration::from_millis(80))
            .await;
    assert!(matches!(
        result,
        Err(crate::runtime::AuthorizedRequestError::SendStartTimeout)
    ));
}

#[tokio::test]
async fn http3_send_start_deadline_preserves_payload_and_quiet_generation() {
    let mut server = Server::with_response_delay(false, Duration::from_millis(600)).await;
    let transport = Http3Transport::with_enabled(true);
    let client = server.client();
    let mut generation = request(&server.url);
    assert!(
        transport
            .select_client(&mut generation, true, "route", &client)
            .is_none()
    );
    assert_eq!(server.next_request().await.method, Method::HEAD);
    wait_probe(&transport, &server.url, "route").await;
    let client = transport
        .select_client(&mut generation, true, "route", &client)
        .unwrap();
    let response =
        super::send::execute_with_start_timeout(&client, generation, Duration::from_millis(200))
            .await
            .unwrap();
    assert_eq!(response.version(), Version::HTTP_3);
    assert_eq!(response.text().await.unwrap(), "synthetic answer");
    let observed = server.next_request().await;
    assert_eq!(observed.body, b"synthetic body");
    assert_eq!(observed.headers["content-length"], "14");
    assert_eq!(observed.headers["authorization"], "Bearer synthetic");
    assert!(
        server.observed.try_recv().is_err(),
        "no replay after dispatch"
    );
}

#[tokio::test]
async fn http3_probe_is_private_single_flight_and_requests_preserve_their_body() {
    let mut server = Server::new(false).await;
    let transport = Http3Transport::with_enabled(true);
    let client = server.client();
    let mut first = request(&format!("{}?private=not-probed", server.url));
    for _ in 0..8 {
        assert!(
            transport
                .select_client(&mut first, true, "route", &client)
                .is_none()
        );
    }
    assert_eq!(first.version(), Version::HTTP_11);
    let probe = server.next_request().await;
    assert_eq!(probe.method, Method::HEAD);
    assert_eq!(probe.path, "/");
    assert!(!probe.headers.contains_key("authorization"));
    assert!(!probe.headers.contains_key("cookie"));
    assert!(probe.body.is_empty());
    wait_probe(&transport, &server.url, "route").await;

    for _ in 0..2 {
        let mut generation = request(&server.url);
        let client = transport
            .select_client(&mut generation, true, "route", &client)
            .unwrap();
        let response = client.execute(generation).await.unwrap();
        assert_eq!(response.version(), Version::HTTP_3);
        assert_eq!(response.text().await.unwrap(), "synthetic answer");
        let observed = server.next_request().await;
        assert_eq!(observed.method, Method::POST);
        assert_eq!(observed.path, "/v1/responses");
        assert_eq!(observed.headers["authorization"], "Bearer synthetic");
        assert_eq!(observed.headers["cookie"], "synthetic=value");
        assert_eq!(observed.body, b"synthetic body");
    }
    assert_eq!(
        server.connections.load(Ordering::SeqCst),
        1,
        "reuse the probed QUIC connection"
    );
    assert!(
        server.observed.try_recv().is_err(),
        "no extra probes or duplicate generation"
    );
}

#[tokio::test]
async fn http3_failed_probe_does_not_delay_or_change_the_generation() {
    let socket = tokio::net::UdpSocket::bind("[::1]:0").await.unwrap();
    let url = format!(
        "https://[::1]:{}/v1/responses",
        socket.local_addr().unwrap().port()
    );
    let transport = Http3Transport::with_enabled(true);
    let client = test_client();
    let mut generation = request(&url);
    assert!(
        transport
            .select_client(&mut generation, true, "route", &client)
            .is_none()
    );
    assert_eq!(generation.version(), Version::HTTP_11);
    assert_eq!(
        generation.body().unwrap().as_bytes().unwrap(),
        b"synthetic body"
    );
    wait_probe(&transport, &url, "route").await;
    let origin = (
        "route".to_string(),
        generation.url().origin().ascii_serialization(),
    );
    let expires = crate::poison::mutex(&transport.origins)[&origin].expires;
    for _ in 0..3 {
        assert!(
            transport
                .select_client(&mut generation, true, "route", &client)
                .is_none()
        );
    }
    assert_eq!(
        crate::poison::mutex(&transport.origins)[&origin].expires,
        expires,
        "an unsupported origin is not reprobed on each request"
    );
}

#[tokio::test]
async fn http3_rejects_untrusted_certificates() {
    let server = Server::new(false).await;
    let transport = Http3Transport::with_enabled(true);
    let client = test_client();
    let mut generation = request(&server.url);
    assert!(
        transport
            .select_client(&mut generation, true, "route", &client)
            .is_none()
    );
    wait_probe(&transport, &server.url, "route").await;
    assert!(
        transport
            .select_client(&mut generation, true, "route", &client)
            .is_none()
    );
    assert_eq!(generation.version(), Version::HTTP_11);
}

#[tokio::test]
async fn http3_reset_after_submission_is_not_replayed_and_next_request_uses_tcp() {
    let mut server = Server::new(true).await;
    let transport = Http3Transport::with_enabled(true);
    let client = server.client();
    let mut generation = request(&server.url);
    assert!(
        transport
            .select_client(&mut generation, true, "route", &client)
            .is_none()
    );
    assert_eq!(server.next_request().await.method, Method::HEAD);
    wait_probe(&transport, &server.url, "route").await;
    let client = transport
        .select_client(&mut generation, true, "route", &client)
        .unwrap();
    let error = client.execute(generation).await.unwrap_err();
    assert!(
        !error.is_connect(),
        "an accepted H3 stream has uncertain execution"
    );
    transport.failed("route", &Url::parse(&server.url).unwrap());
    assert_eq!(server.next_request().await.method, Method::POST);
    let mut next = request(&server.url);
    assert!(
        transport
            .select_client(&mut next, true, "route", &client)
            .is_none()
    );
    assert_eq!(next.version(), Version::HTTP_11);
    assert!(server.observed.try_recv().is_err());
}

#[tokio::test]
async fn http3_preserves_account_connection_and_failure_isolation() {
    let mut server = Server::new(false).await;
    let transport = Http3Transport::with_enabled(true);
    let first = server.client();
    let second = server.client();
    for (candidate, client) in [("account-a", &first), ("account-b", &second)] {
        let mut generation = request(&server.url);
        assert!(
            transport
                .select_client(&mut generation, true, candidate, client)
                .is_none()
        );
        assert_eq!(server.next_request().await.method, Method::HEAD);
        wait_probe(&transport, &server.url, candidate).await;
    }
    assert_eq!(server.connections.load(Ordering::SeqCst), 2);
    transport.failed("account-a", &Url::parse(&server.url).unwrap());
    assert!(
        transport
            .select_client(&mut request(&server.url), true, "account-a", &first)
            .is_none()
    );
    let mut generation = request(&server.url);
    let client = transport
        .select_client(&mut generation, true, "account-b", &second)
        .unwrap();
    let response = client.execute(generation).await.unwrap();
    assert_eq!(response.version(), Version::HTTP_3);
    assert_eq!(response.text().await.unwrap(), "synthetic answer");
    assert_eq!(server.next_request().await.method, Method::POST);
}

fn test_client() -> Client {
    super::super::support::runtime_client_builder(None)
        .no_proxy()
        .tls_early_data(false)
        .http3_max_idle_timeout(Duration::ZERO)
        .build()
        .unwrap()
}

async fn wait_probe(transport: &Http3Transport, url: &str, candidate: &str) {
    let origin = (
        candidate.to_owned(),
        Url::parse(url).unwrap().origin().ascii_serialization(),
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if crate::poison::mutex(&transport.origins)
                .get(&origin)
                .is_some_and(|entry| entry.capability != Capability::Probing)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bounded QUIC probe finishes");
}

struct Observed {
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
}

struct Server {
    url: String,
    cert: reqwest::Certificate,
    endpoint: quinn::Endpoint,
    task: tokio::task::JoinHandle<()>,
    observed: tokio::sync::mpsc::UnboundedReceiver<Observed>,
    connections: Arc<AtomicUsize>,
}

impl Server {
    async fn new(reset_post: bool) -> Self {
        Self::with_response_delay(reset_post, Duration::ZERO).await
    }

    async fn with_response_delay(reset_post: bool, delay: Duration) -> Self {
        let certificate = rcgen::generate_simple_self_signed(vec!["::1".into()]).unwrap();
        let cert = reqwest::Certificate::from_der(certificate.cert.der()).unwrap();
        let key =
            rustls::pki_types::PrivatePkcs8KeyDer::from(certificate.signing_key.serialize_der());
        let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![certificate.cert.der().clone()], key.into())
        .unwrap();
        tls.alpn_protocols = vec![b"h3".to_vec()];
        let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap();
        let endpoint = quinn::Endpoint::server(
            quinn::ServerConfig::with_crypto(Arc::new(crypto)),
            "[::1]:0".parse().unwrap(),
        )
        .unwrap();
        let url = format!(
            "https://[::1]:{}/v1/responses",
            endpoint.local_addr().unwrap().port()
        );
        let acceptor = endpoint.clone();
        let connections = Arc::new(AtomicUsize::new(0));
        let count = connections.clone();
        let (tx, observed) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            while let Some(incoming) = acceptor.accept().await {
                let tx = tx.clone();
                let count = count.clone();
                tasks.spawn(async move {
                    let Ok(connection) = incoming.await else {
                        return;
                    };
                    count.fetch_add(1, Ordering::SeqCst);
                    let mut h3 = h3::server::Connection::<_, Bytes>::new(
                        h3_quinn::Connection::new(connection.clone()),
                    )
                    .await
                    .unwrap();
                    while let Ok(Some(resolver)) = h3.accept().await {
                        let (request, mut stream) = resolver.resolve_request().await.unwrap();
                        let head = request.method() == Method::HEAD;
                        let mut body = Vec::new();
                        while let Some(mut chunk) = stream.recv_data().await.unwrap() {
                            body.extend_from_slice(&chunk.copy_to_bytes(chunk.remaining()));
                        }
                        tx.send(Observed {
                            method: request.method().clone(),
                            path: request.uri().path_and_query().unwrap().as_str().to_owned(),
                            headers: request.headers().clone(),
                            body,
                        })
                        .unwrap();
                        if !head && reset_post {
                            connection.close(1_u32.into(), b"synthetic reset after dispatch");
                            break;
                        }
                        if !head {
                            tokio::time::sleep(delay).await;
                        }
                        // A denied root path still establishes H3 transport capability.
                        stream
                            .send_response(
                                Response::builder()
                                    .status(if head { 405 } else { 200 })
                                    .body(())
                                    .unwrap(),
                            )
                            .await
                            .unwrap();
                        if !head {
                            stream
                                .send_data(Bytes::from_static(b"synthetic answer"))
                                .await
                                .unwrap();
                        }
                        stream.finish().await.unwrap();
                    }
                });
            }
        });
        Self {
            url,
            cert,
            endpoint,
            task,
            observed,
            connections,
        }
    }

    fn client(&self) -> Client {
        super::super::support::runtime_client_builder(None)
            .no_proxy()
            .tls_early_data(false)
            .http3_max_idle_timeout(Duration::ZERO)
            .tls_certs_only([self.cert.clone()])
            .build()
            .unwrap()
    }

    async fn next_request(&mut self) -> Observed {
        tokio::time::timeout(Duration::from_secs(5), self.observed.recv())
            .await
            .unwrap()
            .unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.endpoint.close(0_u32.into(), b"fixture finished");
        self.task.abort();
    }
}

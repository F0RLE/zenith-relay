use super::*;

mod tls_proxy;

#[tokio::test]
async fn loopback_http2_diagnostic_preserves_physical_dispatch_and_retry_limits() {
    let result = diagnose_http2_retries().await.unwrap();
    assert!(result.passed, "{result:?}");
    assert!(result.default_client_requests > 1);
    assert_eq!(result.cases.len(), 4);
    for case in &result.cases {
        assert!(case.passed, "{case:?}");
        assert_eq!(case.physical_requests, case.recorded_attempts);
        assert!(case.physical_requests <= case.candidate_limit);
        assert!(case.one_logical_request);
    }
    let encoded = serde_json::to_string(&result).unwrap();
    for private in [
        LOCAL_KEY,
        MODEL,
        "synthetic-upstream-key",
        "127.0.0.1",
        "requestId",
        "candidateId",
    ] {
        assert!(
            !encoded.contains(private),
            "diagnostic output must contain only aggregate observations"
        );
    }
}

#[tokio::test]
async fn cancelling_a_diagnostic_releases_its_loopback_listener() {
    let mut tasks = JoinSet::new();
    let fixture = Fixture::start(&mut tasks, Reply::Refused).await.unwrap();
    let address: std::net::SocketAddr = fixture
        .url
        .strip_prefix("http://")
        .unwrap()
        .strip_suffix("/v1")
        .unwrap()
        .parse()
        .unwrap();
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    assert_eq!(listener.local_addr().unwrap(), address);
}

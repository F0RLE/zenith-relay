use super::*;
use std::sync::atomic::AtomicUsize;

mod fixture;
use fixture::{ConnectProxy, TlsPeer};

fn tls_runtime(
    peer: &TlsPeer,
    proxy: Option<&ConnectProxy>,
    candidate_limit: usize,
    events: &Arc<Mutex<Vec<UsageEvent>>>,
) -> GatewayRuntime {
    let captured = events.clone();
    let mut runtime = GatewayRuntime::from_pool(
        ["diagnostic-a", "diagnostic-b"]
            .into_iter()
            .map(|id| {
                RuntimeSource::unrestricted(ProviderSource {
                    id: id.into(),
                    name: "Synthetic TLS transport source".into(),
                    base_url: peer.url.clone(),
                    api_key: "synthetic-upstream-key".into(),
                    proxy_url: proxy.map(|proxy| proxy.url.clone()),
                    wire_api: WireApi::Responses,
                    models: vec![MODEL.into()],
                })
            })
            .collect(),
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "diagnostic-key".into(),
            secret: LOCAL_KEY.into(),
        })],
        GatewayRuntimeOptions {
            max_retry_candidates: candidate_limit,
            diagnostics_enabled: || true,
            ..Default::default()
        },
        Arc::new(move |event| crate::poison::mutex(&captured).push(event)),
    )
    .unwrap();
    let candidates = runtime
        .source_state
        .candidate_bindings
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    for candidate in candidates {
        let configured_proxy = runtime.request_proxy(&candidate);
        assert_eq!(configured_proxy.is_some(), proxy.is_some());
        // Trust only this in-memory certificate; keep the production retry and
        // source-proxy policies. No prior knowledge or certificate bypass is used.
        let client = peer.client(configured_proxy, false);
        runtime.source_state.http_clients.insert(candidate, client);
    }
    runtime
}

async fn dispatch(tasks: &mut JoinSet<()>, runtime: GatewayRuntime) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tasks.spawn(async move {
        axum::serve(listener, crate::gateway::router(Arc::new(runtime)))
            .await
            .unwrap();
    });
    gateway_response(address).await.unwrap()
}

async fn stop(tasks: &mut JoinSet<()>) {
    tasks.abort_all();
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            assert!(error.is_cancelled(), "transport fixture panicked: {error}");
        }
    }
}

#[tokio::test]
async fn tls_alpn_and_connect_proxy_keep_physical_sends_within_gateway_budgets() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let mut tasks = JoinSet::new();
        for proxied in [false, true] {
            let control = TlsPeer::start(&mut tasks, Reply::Refused).await;
            let proxy = if proxied {
                Some(ConnectProxy::start(&mut tasks, control.address).await)
            } else {
                None
            };
            let proxy_config = proxy.as_ref().map(ConnectProxy::config);
            let result = control
                .client(proxy_config.as_ref(), true)
                .post(&control.url)
                .body("synthetic request")
                .send()
                .await;
            assert!(result.is_err());
            assert_eq!(
                control.requests.load(Ordering::SeqCst),
                3,
                "proxied={proxied}"
            );
            assert!(control.connections.load(Ordering::SeqCst) > 0);
            if let Some(proxy) = proxy {
                assert_eq!(
                    proxy.tunnels.load(Ordering::SeqCst),
                    control.connections.load(Ordering::SeqCst)
                );
            }

            for (name, reply, limit, expected) in [
                ("refused_stream", Reply::Refused, 1, 1),
                ("uncertain_stream_reset", Reply::Cancelled, 2, 1),
                (
                    "rate_limit_budget_exhausted",
                    Reply::RateLimitThenSuccess,
                    1,
                    1,
                ),
                ("rate_limit_failover", Reply::RateLimitThenSuccess, 2, 2),
            ] {
                let peer = TlsPeer::start(&mut tasks, reply).await;
                let proxy = if proxied {
                    Some(ConnectProxy::start(&mut tasks, peer.address).await)
                } else {
                    None
                };
                let events = Arc::new(Mutex::new(Vec::new()));
                let runtime = tls_runtime(&peer, proxy.as_ref(), limit, &events);
                let status = dispatch(&mut tasks, runtime).await;
                let physical = peer.requests.load(Ordering::SeqCst);
                let case = DiagnosticCase {
                    name,
                    reply,
                    candidate_limit: limit,
                    expected_requests: expected,
                };
                let observation = observe_events(
                    &case,
                    &crate::poison::mutex(&events),
                    physical,
                    status,
                    false,
                );
                assert!(observation.passed, "proxied={proxied}: {observation:?}");
                assert!(peer.connections.load(Ordering::SeqCst) > 0);
                if let Some(proxy) = proxy {
                    assert_eq!(
                        proxy.tunnels.load(Ordering::SeqCst),
                        peer.connections.load(Ordering::SeqCst)
                    );
                }
            }
        }
        stop(&mut tasks).await;
    })
    .await
    .expect("TLS/CONNECT retry qualification exceeded its bounded deadline");
}

#[tokio::test]
async fn unavailable_tls_proxy_never_bypasses_to_the_direct_peer() {
    let mut tasks = JoinSet::new();
    let peer = TlsPeer::start(&mut tasks, Reply::RateLimitThenSuccess).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = ConnectProxy {
        url: format!("http://{}", listener.local_addr().unwrap()),
        tunnels: Arc::new(AtomicUsize::new(0)),
    };
    drop(listener);
    let events = Arc::new(Mutex::new(Vec::new()));
    let runtime = tls_runtime(&peer, Some(&proxy), 1, &events);
    assert_ne!(dispatch(&mut tasks, runtime).await, 200);
    assert_eq!(peer.requests.load(Ordering::SeqCst), 0);
    assert_eq!(peer.connections.load(Ordering::SeqCst), 0);
    {
        let events = crate::poison::mutex(&events);
        let attempts = events
            .iter()
            .filter(|event| !event.is_request_completion())
            .collect::<Vec<_>>();
        assert_eq!(attempts.len(), 1);
        assert!(!attempts[0].success);
        assert!(attempts[0].total_tokens.is_none());
    }
    stop(&mut tasks).await;
}

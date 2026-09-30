use super::*;

fn gate(concurrent: usize, per_origin: usize, max_waiters: usize) -> Arc<ManagementHttpGate> {
    ManagementHttpGate::new(HttpLimits {
        concurrent,
        per_origin,
        reserved_auth: 1,
        max_waiters,
        max_wait: Duration::from_millis(150),
    })
    .unwrap()
}

fn url(origin: &str) -> Url {
    Url::parse(origin).unwrap()
}

async fn queued(gate: &ManagementHttpGate, count: usize) {
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if gate.state.lock().unwrap().waiters.len() == count {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn total_and_origin_limits_reserve_auth_without_blocking_other_origins() {
    let gate = gate(3, 2, 4);
    let a = url("https://a.test/private?key=not-retained");
    let b = url("https://b.test/");
    let ordinary_a = gate.acquire(&a, HttpClass::Ordinary).await.unwrap();
    let ordinary_b = gate.acquire(&b, HttpClass::Ordinary).await.unwrap();

    let third = gate.acquire(&a, HttpClass::Auth).await.unwrap();
    assert_eq!(gate.state.lock().unwrap().total, 3);
    assert_eq!(gate.state.lock().unwrap().origins.len(), 2);
    assert_eq!(
        gate.state.lock().unwrap().origins.keys().next().unwrap(),
        "https://a.test"
    );

    let waiting = tokio::spawn({
        let gate = gate.clone();
        let a = a.clone();
        async move { gate.acquire(&a, HttpClass::Auth).await }
    });
    queued(&gate, 1).await;
    drop(ordinary_b);
    // Total capacity opened, but this origin remains full. No permit leaks.
    assert_eq!(
        waiting.await.unwrap().err(),
        Some(HttpAdmissionError::TimedOut)
    );
    drop(third);
    drop(ordinary_a);
    assert!(gate.state.lock().unwrap().origins.is_empty());
}

#[tokio::test]
async fn feasible_fifo_fairness_and_auth_reservation() {
    let gate = gate(2, 2, 4);
    let a = url("https://a.test/");
    let b = url("https://b.test/");
    let ordinary = gate.acquire(&a, HttpClass::Ordinary).await.unwrap();
    let blocked_a = tokio::spawn({
        let gate = gate.clone();
        let a = a.clone();
        async move { gate.acquire(&a, HttpClass::Ordinary).await }
    });
    queued(&gate, 1).await;
    let auth_b = gate.acquire(&b, HttpClass::Auth).await.unwrap();
    let blocked_auth = tokio::spawn({
        let gate = gate.clone();
        let b = b.clone();
        async move { gate.acquire(&b, HttpClass::Auth).await }
    });
    queued(&gate, 2).await;
    drop(auth_b);
    let auth_b = blocked_auth.await.unwrap().unwrap();
    assert!(!blocked_a.is_finished());
    drop(ordinary);
    let ordinary_a = blocked_a.await.unwrap().unwrap();
    drop(auth_b);
    drop(ordinary_a);
}

#[tokio::test]
async fn bounded_waiters_cancel_and_timeout_release_metadata() {
    let gate = gate(2, 2, 1);
    let a = url("https://a.test/");
    let ordinary = gate.acquire(&a, HttpClass::Ordinary).await.unwrap();
    let auth = gate.acquire(&a, HttpClass::Auth).await.unwrap();
    let waiting = tokio::spawn({
        let gate = gate.clone();
        let a = a.clone();
        async move { gate.acquire(&a, HttpClass::Auth).await }
    });
    queued(&gate, 1).await;
    assert_eq!(
        gate.acquire(&a, HttpClass::Auth).await.err(),
        Some(HttpAdmissionError::Full)
    );
    waiting.abort();
    let _ = waiting.await;
    queued(&gate, 0).await;
    assert_eq!(
        gate.acquire(&a, HttpClass::Auth).await.err(),
        Some(HttpAdmissionError::TimedOut)
    );
    queued(&gate, 0).await;
    drop(auth);
    drop(ordinary);
    assert!(gate.state.lock().unwrap().origins.is_empty());
}

#[tokio::test]
async fn ordinary_waiters_cannot_fill_the_auth_recovery_slot() {
    let gate = gate(2, 2, 2);
    let a = url("https://a.test/");
    let ordinary = gate.acquire(&a, HttpClass::Ordinary).await.unwrap();
    let auth = gate.acquire(&a, HttpClass::Auth).await.unwrap();
    let ordinary_waiter = tokio::spawn({
        let (gate, a) = (gate.clone(), a.clone());
        async move { gate.acquire(&a, HttpClass::Ordinary).await }
    });
    queued(&gate, 1).await;
    assert_eq!(
        gate.acquire(&a, HttpClass::Ordinary).await.err(),
        Some(HttpAdmissionError::Full)
    );
    let auth_waiter = tokio::spawn({
        let (gate, a) = (gate.clone(), a.clone());
        async move { gate.acquire(&a, HttpClass::Auth).await }
    });
    queued(&gate, 2).await;
    drop(auth);
    let recovered = auth_waiter.await.unwrap().unwrap();
    assert!(!ordinary_waiter.is_finished());
    drop(recovered);
    drop(ordinary);
    drop(ordinary_waiter.await.unwrap().unwrap());
    assert!(gate.state.lock().unwrap().origins.is_empty());
}

#[tokio::test]
async fn no_url_credentials_or_unbounded_origin_are_retained() {
    let gate = gate(2, 2, 1);
    for invalid in ["file:///private", "https://user:password@a.test/"] {
        assert_eq!(
            gate.acquire(&url(invalid), HttpClass::Auth).await.err(),
            Some(HttpAdmissionError::InvalidOrigin)
        );
    }
    assert!(gate.state.lock().unwrap().origins.is_empty());
}

#[tokio::test]
async fn send_holds_the_permit_until_the_response_body_is_read() {
    use axum::{body::Body, routing::get, Router};
    use futures_util::stream;
    use std::convert::Infallible;
    use tokio::sync::Notify;

    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let app = Router::new()
        .route(
            "/slow",
            get({
                let (started, release) = (started.clone(), release.clone());
                move || {
                    let (started, release) = (started.clone(), release.clone());
                    async move {
                        Body::from_stream(stream::once(async move {
                            started.notify_one();
                            release.notified().await;
                            Ok::<_, Infallible>(axum::body::Bytes::from_static(b"complete"))
                        }))
                    }
                }
            }),
        )
        .route("/fast", get(|| async { "fast" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    let gate = gate(2, 2, 4);
    let (slow, permit) = gate
        .send(
            &client,
            client.get(format!("http://{address}/slow")),
            HttpClass::Ordinary,
        )
        .await
        .unwrap();
    let slow_body = tokio::spawn(async move {
        let bytes = slow.bytes().await.unwrap();
        drop(permit);
        bytes
    });
    started.notified().await;
    let fast_body = tokio::spawn({
        let (client, gate) = (client.clone(), gate.clone());
        async move {
            let (response, permit) = gate
                .send(
                    &client,
                    client.get(format!("http://{address}/fast")),
                    HttpClass::Ordinary,
                )
                .await
                .unwrap();
            let body = response.bytes().await.unwrap();
            drop(permit);
            body
        }
    });
    queued(&gate, 1).await;
    assert!(!fast_body.is_finished());
    release.notify_one();
    assert_eq!(&slow_body.await.unwrap()[..], b"complete");
    assert_eq!(&fast_body.await.unwrap()[..], b"fast");
    assert!(gate.state.lock().unwrap().origins.is_empty());
    server.abort();
}

#[tokio::test]
async fn post_401_retry_needs_a_new_permit() {
    use axum::{http::StatusCode, routing::get, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};

    let attempts = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/auth",
        get({
            let attempts = attempts.clone();
            move || {
                let attempts = attempts.clone();
                async move {
                    if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                        (StatusCode::UNAUTHORIZED, "rejected")
                    } else {
                        (StatusCode::OK, "accepted")
                    }
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let gate = gate(2, 2, 2);
    let client = reqwest::Client::new();
    let endpoint = format!("http://{address}/auth");
    for status in [reqwest::StatusCode::UNAUTHORIZED, reqwest::StatusCode::OK] {
        let (response, permit) = gate
            .send(&client, client.get(&endpoint), HttpClass::Ordinary)
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        response.bytes().await.unwrap();
        drop(permit);
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert!(gate.state.lock().unwrap().origins.is_empty());
    server.abort();
}

#[tokio::test]
async fn revision_changed_while_waiting_never_reaches_the_http_handler() {
    use axum::{routing::get, Router};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let hits = Arc::new(AtomicUsize::new(0));
    let app = Router::new().route(
        "/refresh",
        get({
            let hits = hits.clone();
            move || {
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    "unexpected"
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let endpoint = format!("http://{address}/refresh");
    let gate = gate(2, 2, 2);
    let blocker = gate
        .acquire(&url(&endpoint), HttpClass::Ordinary)
        .await
        .unwrap();
    let current = Arc::new(AtomicBool::new(true));
    let pending = tokio::spawn({
        let (gate, current) = (gate.clone(), current.clone());
        async move {
            let client = reqwest::Client::new();
            gate.send_if_current(&client, client.get(endpoint), HttpClass::Ordinary, || {
                current.load(Ordering::SeqCst)
            })
            .await
        }
    });
    queued(&gate, 1).await;
    current.store(false, Ordering::SeqCst);
    drop(blocker);
    assert!(matches!(pending.await.unwrap(), Err(HttpSendError::Stale)));
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert!(gate.state.lock().unwrap().origins.is_empty());
    server.abort();
}

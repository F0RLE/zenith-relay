use super::*;
use zenith_relay_core::accounts::TokenRefreshFailureKind;

const ACCOUNT_ID: &str = "synthetic-native-auth";

#[derive(Default)]
struct HeldRefresh {
    entered: Notify,
    release: Notify,
    calls: AtomicUsize,
    failure: Mutex<Option<TokenRefreshFailureKind>>,
}

impl TokenRefreshAdapter for HeldRefresh {
    fn refresh<'a>(
        &'a self,
        account_id: &'a str,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async move {
            assert_eq!(account_id, ACCOUNT_ID);
            assert_eq!(refresh_token, "synthetic-refresh");
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            self.release.notified().await;
            if let Some(kind) = *self.failure.lock().unwrap() {
                return Err(TokenRefreshFailure::new(kind, "synthetic_refresh_failure"));
            }
            Ok(TokenRefresh::new(
                "synthetic-refreshed",
                Some("synthetic-rotated".into()),
                None,
                Some(now_ms + 600_000),
            )
            .unwrap())
        })
    }
}

async fn register_expired(authority: &TokenAuthority) {
    let now = current_time_ms();
    authority
        .register(
            ACCOUNT_ID,
            TokenSet::new(
                "synthetic-expired",
                Some("synthetic-refresh".into()),
                None,
                Some(now - 1),
                now - 1_000,
                1,
            )
            .unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
}

struct AuthorizationFixture {
    _upstream: TestServer,
    gateway: TestServer,
    probe: NativeProbe,
    authority: Arc<TokenAuthority>,
    refresh: Arc<HeldRefresh>,
    persistence: Arc<PersistenceAdapter>,
    events: Arc<Mutex<Vec<UsageEvent>>>,
}

impl AuthorizationFixture {
    async fn new() -> Self {
        let (upstream, probe) = fixture_with_startup(false).await;
        let authority = ready_authority(ACCOUNT_ID, "synthetic-old").await;
        let refresh = Arc::new(HeldRefresh::default());
        let persistence = Arc::new(PersistenceAdapter::default());
        let (gateway, events, _, _) = spawn_mixed_gateway(
            vec![],
            vec![account(ACCOUNT_ID, "synthetic-provider", &upstream, 10)],
            vec![mixed_key(None, None)],
            authority.clone(),
            refresh.clone(),
            persistence.clone(),
        )
        .await;
        Self {
            _upstream: upstream,
            gateway,
            probe,
            authority,
            refresh,
            persistence,
            events,
        }
    }

    async fn pending(&self) -> reqwest_websocket::WebSocket {
        let mut socket = connect(&self.gateway).await;
        socket.send(create(0, Some("active"))).await.unwrap();
        self.probe.wait_requests(1).await;
        self.probe.emit(
            json!({"type":"response.output_text.delta", "stream_id":"active",
            "response_id":"native-0", "delta":"synthetic startup"}),
        );
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.output_text.delta"
        );
        register_expired(&self.authority).await;
        socket.send(create(1, Some("pending"))).await.unwrap();
        self.wait_refresh().await;
        socket
    }

    async fn wait_refresh(&self) {
        tokio::time::timeout(Duration::from_secs(3), self.refresh.entered.notified())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn websocket_native_authorization_refresh_does_not_block_an_active_lane() {
    let fixture = AuthorizationFixture::new().await;
    let mut socket = fixture.pending().await;
    let AuthorizationFixture {
        probe,
        gateway,
        refresh,
        persistence,
        events,
        ..
    } = &fixture;
    probe.complete(Some("active"), "native-0", 2);
    let completed = receive_websocket_json(&mut socket).await;
    assert_eq!(completed["type"], "response.completed");
    assert_eq!(completed["stream_id"], "active");
    assert_eq!(probe.requests.lock().unwrap().len(), 1);
    refresh.release.notify_one();
    probe.wait_requests(2).await;
    assert_eq!(probe.sockets.lock().unwrap().len(), 2);
    probe.complete(Some("pending"), "native-1", 3);
    assert_eq!(
        receive_websocket_json(&mut socket).await["stream_id"],
        "pending"
    );
    probe.wait_settled(gateway).await;
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
    assert_eq!(persistence.token_writes.load(Ordering::SeqCst), 1);
    let headers = probe.headers.lock().unwrap();
    assert_eq!(
        header(&headers[0], "authorization").as_deref(),
        Some("Bearer synthetic-old")
    );
    assert_eq!(
        header(&headers[1], "authorization").as_deref(),
        Some("Bearer synthetic-refreshed")
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| event.success && event.attempt == 1)
    );
}

#[tokio::test]
async fn websocket_native_authorization_disconnect_releases_lease_before_rotated_grant_persists() {
    let fixture = AuthorizationFixture::new().await;
    let mut socket = fixture.pending().await;
    socket
        .send(ClientWsMessage::Close {
            code: reqwest_websocket::CloseCode::Normal,
            reason: String::new(),
        })
        .await
        .unwrap();
    fixture.probe.wait_settled(&fixture.gateway).await;
    assert_eq!(fixture.persistence.token_writes.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.probe.requests.lock().unwrap().len(), 1);
    fixture.refresh.release.notify_one();
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.persistence.token_writes.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fixture.refresh.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.probe.sockets.lock().unwrap().len(), 1);
    let events = fixture.events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(events[0].attempt, 1);
}

#[tokio::test]
async fn websocket_native_authorization_failure_does_not_cancel_an_active_lane() {
    for kind in [
        TokenRefreshFailureKind::Transient,
        TokenRefreshFailureKind::InvalidGrant,
    ] {
        let fixture = AuthorizationFixture::new().await;
        let mut socket = fixture.pending().await;
        *fixture.refresh.failure.lock().unwrap() = Some(kind);
        fixture.refresh.release.notify_one();
        let error = receive_websocket_json(&mut socket).await;
        assert_eq!(error["type"], "error");
        assert_eq!(error["stream_id"], "pending");
        fixture.probe.complete(Some("active"), "native-0", 2);
        let complete = receive_websocket_json(&mut socket).await;
        assert_eq!(complete["type"], "response.completed");
        assert_eq!(complete["stream_id"], "active");
        fixture.probe.wait_settled(&fixture.gateway).await;
        assert_eq!(fixture.probe.requests.lock().unwrap().len(), 1);
        assert_eq!(fixture.probe.sockets.lock().unwrap().len(), 1);
        assert_eq!(fixture.persistence.token_writes.load(Ordering::SeqCst), 0);
        if kind == TokenRefreshFailureKind::InvalidGrant {
            assert_eq!(
                fixture.authority.auth_state(ACCOUNT_ID).await,
                Some(AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant))
            );
            assert!(
                !fixture
                    .gateway
                    .runtime
                    .as_ref()
                    .unwrap()
                    .candidate_runtime_order()[0]
                    .available
            );
        }
        let events = fixture.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert!(
            events
                .iter()
                .any(|event| !event.success && event.attempt == 0)
        );
        assert!(
            events
                .iter()
                .any(|event| event.success && event.attempt == 1 && event.input_tokens == Some(2))
        );
    }
}

#[tokio::test]
async fn websocket_native_authorization_handoff_checks_history_before_replacing_a_drained_socket() {
    for portable in [false, true] {
        let fixture = AuthorizationFixture::new().await;
        let mut socket = connect(&fixture.gateway).await;
        socket.send(create(0, Some("parent"))).await.unwrap();
        fixture.probe.wait_requests(1).await;
        let output = if portable {
            json!([{"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":"synthetic parent"}]}])
        } else {
            json!([{"type":"compaction", "encrypted_content":"synthetic-opaque-state"}])
        };
        fixture
            .probe
            .emit(json!({"type":"response.completed", "stream_id":"parent",
            "response":{"id":"native-0", "status":"completed", "output":output}}));
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.completed"
        );
        continuation(&mut socket, "active", "synthetic active").await;
        fixture.probe.wait_requests(2).await;
        assert_eq!(fixture.probe.sockets.lock().unwrap().len(), 1);
        fixture.probe.emit(
            json!({"type":"response.output_text.delta", "stream_id":"active",
            "response_id":"native-1", "delta":"synthetic active"}),
        );
        assert_eq!(
            receive_websocket_json(&mut socket).await["stream_id"],
            "active"
        );
        register_expired(&fixture.authority).await;
        continuation(&mut socket, "pending", "synthetic follow-up").await;
        fixture.wait_refresh().await;
        fixture.refresh.release.notify_one();
        fixture.probe.complete(Some("active"), "native-1", 2);
        assert_eq!(
            receive_websocket_json(&mut socket).await["stream_id"],
            "active"
        );
        if portable {
            fixture.probe.wait_requests(3).await;
            assert_eq!(fixture.probe.sockets.lock().unwrap().len(), 2);
            {
                let requests = fixture.probe.requests.lock().unwrap();
                assert!(requests[2].get("previous_response_id").is_none());
                assert!(
                    requests[2]["input"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|item| item["role"] == "assistant")
                );
            }
            fixture.probe.complete(Some("pending"), "native-2", 3);
            assert_eq!(
                receive_websocket_json(&mut socket).await["type"],
                "response.completed"
            );
        } else {
            let error = receive_websocket_json(&mut socket).await;
            assert_eq!(error["type"], "error");
            assert_eq!(error["stream_id"], "pending");
            assert_eq!(error["error"]["code"], "response_continuation_unavailable");
            assert_eq!(fixture.probe.requests.lock().unwrap().len(), 2);
            assert_eq!(fixture.probe.sockets.lock().unwrap().len(), 1);
        }
        fixture.probe.wait_settled(&fixture.gateway).await;
        assert_eq!(fixture.refresh.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.persistence.token_writes.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn websocket_native_authorization_obsolete_refresh_cannot_poison_a_readded_login() {
    for failure in [None, Some(TokenRefreshFailureKind::InvalidGrant)] {
        let fixture = AuthorizationFixture::new().await;
        let mut socket = fixture.pending().await;
        fixture.probe.complete(Some("active"), "native-0", 2);
        assert_eq!(
            receive_websocket_json(&mut socket).await["stream_id"],
            "active"
        );
        assert!(fixture.authority.remove(ACCOUNT_ID));
        let now = current_time_ms();
        fixture
            .authority
            .register(
                ACCOUNT_ID,
                TokenSet::access_only("synthetic-new-login", Some(now + 600_000), now).unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        *fixture.refresh.failure.lock().unwrap() = failure;
        fixture.refresh.release.notify_one();
        fixture.probe.wait_requests(2).await;
        assert_eq!(fixture.probe.sockets.lock().unwrap().len(), 2);
        fixture.probe.complete(Some("pending"), "native-1", 3);
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.completed"
        );
        fixture.probe.wait_settled(&fixture.gateway).await;
        assert_eq!(fixture.refresh.calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.persistence.token_writes.load(Ordering::SeqCst), 0);
        assert_eq!(
            fixture
                .authority
                .tokens(ACCOUNT_ID)
                .await
                .unwrap()
                .access_token(),
            "synthetic-new-login"
        );
        assert_eq!(
            header(&fixture.probe.headers.lock().unwrap()[1], "authorization").as_deref(),
            Some("Bearer synthetic-new-login")
        );
        assert!(
            fixture
                .events
                .lock()
                .unwrap()
                .iter()
                .all(|event| event.success && event.attempt == 1)
        );
    }
}

#[tokio::test]
async fn websocket_native_authorization_repeated_login_replacement_bounds_preparation() {
    let fixture = AuthorizationFixture::new().await;
    let mut socket = fixture.pending().await;
    fixture.probe.complete(Some("active"), "native-0", 2);
    assert_eq!(
        receive_websocket_json(&mut socket).await["stream_id"],
        "active"
    );
    // The replacement expires after preparation began, but before its retry.
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(fixture.authority.remove(ACCOUNT_ID));
    register_expired(&fixture.authority).await;
    fixture.refresh.release.notify_one();
    fixture.wait_refresh().await;
    assert_eq!(fixture.refresh.calls.load(Ordering::SeqCst), 2);
    assert!(fixture.authority.remove(ACCOUNT_ID));
    let now = current_time_ms();
    fixture
        .authority
        .register(
            ACCOUNT_ID,
            TokenSet::access_only("synthetic-final-login", Some(now + 600_000), now).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    fixture.refresh.release.notify_one();
    let error = receive_websocket_json(&mut socket).await;
    assert_eq!(error["type"], "error");
    assert_eq!(error["stream_id"], "pending");
    fixture.probe.wait_settled(&fixture.gateway).await;
    assert_eq!(fixture.probe.requests.lock().unwrap().len(), 1);
    assert_eq!(fixture.probe.sockets.lock().unwrap().len(), 1);
    assert_eq!(fixture.persistence.token_writes.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture.authority.auth_state(ACCOUNT_ID).await,
        Some(AccountAuthState::Active)
    );
    socket.send(create(2, Some("after"))).await.unwrap();
    fixture.probe.wait_requests(2).await;
    assert_eq!(
        header(&fixture.probe.headers.lock().unwrap()[1], "authorization").as_deref(),
        Some("Bearer synthetic-final-login")
    );
    fixture.probe.complete(Some("after"), "native-2", 3);
    assert_eq!(
        receive_websocket_json(&mut socket).await["stream_id"],
        "after"
    );
    fixture.probe.wait_settled(&fixture.gateway).await;
    assert_eq!(fixture.refresh.calls.load(Ordering::SeqCst), 2);
    let events = fixture.events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events
            .iter()
            .filter(|event| !event.success && event.attempt == 0)
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event.success && event.attempt == 1)
            .count(),
        2
    );
}

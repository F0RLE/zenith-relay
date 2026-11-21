use super::*;
use axum::{
    Router,
    extract::ws::{Message as ServerMessage, WebSocketUpgrade},
    routing::get,
};
use tokio::sync::mpsc;

struct Fixture {
    connection: Option<IdleConnection>,
    events: mpsc::UnboundedReceiver<&'static str>,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/responses", listener.local_addr().unwrap());
        let (events, received) = mpsc::unbounded_channel();
        let router = Router::new().route(
            "/v1/responses",
            get(move |upgrade: WebSocketUpgrade| {
                let events = events.clone();
                async move {
                    upgrade.on_upgrade(move |mut socket| async move {
                        while let Some(Ok(message)) = socket.recv().await {
                            match message {
                                ServerMessage::Ping(_) => {
                                    let _ = events.send("ping");
                                    if socket.flush().await.is_err() {
                                        break;
                                    }
                                }
                                ServerMessage::Pong(_) => {}
                                _ => {
                                    let _ = events.send("data");
                                }
                            }
                        }
                        let _ = events.send("closed");
                    })
                }
            }),
        );
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let socket = transport::upgrade(reqwest::Client::new().get(url))
            .await
            .unwrap()
            .into_websocket()
            .await
            .unwrap();
        Self {
            connection: Some(IdleConnection {
                socket,
                opened: Instant::now(),
                checkpoint: None,
            }),
            events: received,
            server,
        }
    }
}

#[tokio::test]
async fn idle_socket_sends_only_control_heartbeats_and_drops_with_its_owner() {
    let mut fixture = Fixture::start().await;
    tokio::time::pause();
    let parked = idle::IdleSocket::retain(fixture.connection.take().unwrap());
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(20)).await;
    tokio::time::resume();
    assert_eq!(fixture.events.recv().await, Some("ping"));
    drop(parked);
    assert_eq!(fixture.events.recv().await, Some("closed"));
}

#[tokio::test]
async fn same_authority_retains_checkpoint_while_credential_or_incarnation_changes_close_it() {
    use crate::accounts::{AccountAuthState, TokenAuthority, TokenSet};
    use crate::runtime::AuthorizationIncarnation;

    struct NoRefresh;
    impl crate::accounts::TokenRefreshAdapter for NoRefresh {
        fn refresh<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
            _: u64,
        ) -> futures_util::future::BoxFuture<
            'a,
            Result<crate::accounts::TokenRefresh, crate::accounts::TokenRefreshFailure>,
        > {
            Box::pin(async { panic!("synthetic access token must remain usable") })
        }
    }

    for credential_changes in [false, true] {
        let authority = TokenAuthority::new(1).unwrap();
        let tokens = TokenSet::access_only("synthetic-bearer", None, 0).unwrap();
        authority
            .register("account", tokens.clone(), AccountAuthState::Active)
            .await
            .unwrap();
        let prepared = authority
            .prepare("account", 1, 0, &NoRefresh)
            .await
            .unwrap();
        let mut lane = Lane::new();
        lane.bind(SocketAuthority {
            credential: [1; 32],
            incarnation: AuthorizationIncarnation::OAuth(prepared.dispatch_revision),
        });
        assert_eq!(lane.reset_reason, Some(ConnectionResetReason::NewSession));
        let mut fixture = Fixture::start().await;
        let mut connection = fixture.connection.take().unwrap();
        connection.checkpoint = plan(&request(json!([])), None).state.complete(&json!({
            "id":"synthetic-response","status":"completed","output":[{
                "type":"message","role":"assistant","content":[]
            }]
        }));
        lane.idle = Some(idle::IdleSocket::retain(connection));
        lane.reset_reason = None;

        // Normal preparation must not create a new authorization incarnation.
        let prepared = authority
            .prepare("account", 2, 0, &NoRefresh)
            .await
            .unwrap();
        lane.bind(SocketAuthority {
            credential: [1; 32],
            incarnation: AuthorizationIncarnation::OAuth(prepared.dispatch_revision),
        });
        assert!(lane.reset_reason.is_none());
        let connection = lane.idle.take().unwrap().resume().await.unwrap();
        assert!(connection.checkpoint.is_some());
        lane.idle = Some(idle::IdleSocket::retain(connection));

        let tokens = if credential_changes {
            TokenSet::access_only("synthetic-new-bearer", None, 3).unwrap()
        } else {
            tokens
        };
        authority
            .register("account", tokens, AccountAuthState::Active)
            .await
            .unwrap();
        let prepared = authority
            .prepare("account", 4, 0, &NoRefresh)
            .await
            .unwrap();
        lane.bind(SocketAuthority {
            credential: if credential_changes { [2; 32] } else { [1; 32] },
            incarnation: AuthorizationIncarnation::OAuth(prepared.dispatch_revision),
        });
        assert!(lane.idle.is_none());
        let reason = lane.reset_reason.unwrap();
        assert_eq!(
            reason,
            if credential_changes {
                ConnectionResetReason::CredentialChanged
            } else {
                ConnectionResetReason::AuthorizationChanged
            }
        );
        let encoded = serde_json::to_string(&reason).unwrap();
        assert_eq!(
            serde_json::from_str::<ConnectionResetReason>(&encoded).unwrap(),
            reason
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), fixture.events.recv())
                .await
                .unwrap(),
            Some("closed")
        );
    }
}

#[tokio::test]
async fn idle_socket_expires_without_waiting_for_another_request() {
    let mut fixture = Fixture::start().await;
    tokio::time::pause();
    let parked = idle::IdleSocket::retain(fixture.connection.take().unwrap());
    tokio::task::yield_now().await;
    tokio::time::advance(IDLE_TTL).await;
    assert!(matches!(
        parked.resume().await,
        Err(ConnectionResetReason::SessionExpired)
    ));
}

#[tokio::test]
async fn idle_socket_preserves_checkpoint_across_a_fifteen_minute_pause() {
    let mut fixture = Fixture::start().await;
    let mut connection = fixture.connection.take().unwrap();
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    connection.checkpoint = plan(&body, None).state.complete(&json!({
        "id":"synthetic-retained-response","status":"completed","output":[{
            "type":"message","role":"assistant",
            "content":[{"type":"output_text","text":"synthetic"}]
        }]
    }));
    tokio::time::pause();
    let parked = idle::IdleSocket::retain(connection);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(15 * 60)).await;
    let connection = parked
        .resume()
        .await
        .expect("still within the idle lifetime");
    assert!(connection.checkpoint.is_some());
    while let Ok(event) = fixture.events.try_recv() {
        assert_eq!(event, "ping", "idle retention must not send a generation");
    }
}

#[tokio::test]
async fn maximum_connection_age_still_limits_a_parked_socket() {
    let mut fixture = Fixture::start().await;
    let mut connection = fixture.connection.take().unwrap();
    connection.opened = Instant::now() - MAX_AGE;
    tokio::time::pause();
    let parked = idle::IdleSocket::retain(connection);
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(1)).await;
    assert!(matches!(
        parked.resume().await,
        Err(ConnectionResetReason::ConnectionExpired)
    ));
}

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

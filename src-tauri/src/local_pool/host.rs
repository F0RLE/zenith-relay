use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use std::{io::ErrorKind, net::SocketAddr, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::{oneshot, watch, Mutex},
    time::{sleep, timeout, Duration},
};
use tower::ServiceExt;
use zenith_relay_core::GatewayRuntime;

#[cfg(not(test))]
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(100);

// Windows can report an address as still in use for a moment after a clean
// listener shutdown. Retrying this narrow, local condition prevents a normal
// account/source import from being rolled back merely because the previous
// gateway listener is finishing its release.
const BIND_RETRY_DELAY: Duration = Duration::from_millis(50);
const BIND_RETRY_ATTEMPTS: usize = 5;

struct RunningGateway {
    address: SocketAddr,
    routes: watch::Sender<GatewayRoutes>,
    shutdown: oneshot::Sender<()>,
    task: tauri::async_runtime::JoinHandle<()>,
}

struct GatewayRoutes {
    runtime: Arc<GatewayRuntime>,
    router: axum::Router,
}

impl GatewayRoutes {
    fn new(runtime: Arc<GatewayRuntime>) -> Self {
        Self {
            router: zenith_relay_core::gateway::router(runtime.clone()),
            runtime,
        }
    }
}

#[derive(Clone, Default)]
pub struct GatewayManager {
    running: Arc<Mutex<Option<RunningGateway>>>,
}

impl GatewayManager {
    pub async fn start(&self, runtime: Arc<GatewayRuntime>, port: u16) -> Result<SocketAddr> {
        let mut running = self.running.lock().await;
        prune_finished(&mut running);
        if let Some(running_host) = running.as_ref() {
            return Ok(running_host.address);
        }
        let listener = bind_loopback_listener(port).await?;
        let address = listener.local_addr().map_err(|error| {
            LocalPoolError::new(ErrorCode::GatewayUnavailable, error.to_string())
        })?;
        let (shutdown, receiver) = oneshot::channel();
        let (routes, current_routes) = watch::channel(GatewayRoutes::new(runtime));
        // Resolve the current router for every request, not every connection:
        // keep-alive clients must observe changed credentials and membership.
        let service = tower::service_fn(move |request| {
            let router = current_routes.borrow().router.clone();
            async move { router.oneshot(request).await }
        });
        let task = tauri::async_runtime::spawn(async move {
            let _ = axum::serve(listener, axum::Router::new().fallback_service(service))
                .with_graceful_shutdown(async move {
                    let _ = receiver.await;
                })
                .await;
        });
        *running = Some(RunningGateway {
            address,
            routes,
            shutdown,
            task,
        });
        Ok(address)
    }

    /// Publish a fully prepared runtime without draining the listener. Already
    /// dispatched streams keep their router; delayed old dispatches are fenced.
    /// A port change still needs the regular bind/rollback path.
    pub async fn replace_runtime(&self, runtime: Arc<GatewayRuntime>, port: u16) -> bool {
        let mut running = self.running.lock().await;
        prune_finished(&mut running);
        let Some(gateway) = running
            .as_ref()
            .filter(|gateway| gateway.address.port() == port)
        else {
            return false;
        };
        let next = GatewayRoutes::new(runtime);
        gateway.routes.borrow().runtime.retire_for_replacement();
        gateway.routes.send_replace(next);
        true
    }

    pub async fn stop(&self) {
        let running = self.running.lock().await.take();
        if let Some(running) = running {
            let _ = running.shutdown.send(());
            let mut task = running.task;
            if timeout(SHUTDOWN_TIMEOUT, &mut task).await.is_err() {
                task.abort();
                let _ = task.await;
            }
        }
    }

    pub async fn address(&self) -> Option<SocketAddr> {
        let mut running = self.running.lock().await;
        prune_finished(&mut running);
        running.as_ref().map(|running| running.address)
    }

    pub async fn runtime(&self) -> Option<Arc<GatewayRuntime>> {
        let mut running = self.running.lock().await;
        prune_finished(&mut running);
        running
            .as_ref()
            .map(|running| running.routes.borrow().runtime.clone())
    }
}

async fn bind_loopback_listener(port: u16) -> Result<TcpListener> {
    let mut retries = 0;
    loop {
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => return Ok(listener),
            Err(error) if error.kind() == ErrorKind::AddrInUse && retries < BIND_RETRY_ATTEMPTS => {
                retries += 1;
                sleep(BIND_RETRY_DELAY).await;
            }
            Err(error) => {
                return Err(LocalPoolError::new(
                    ErrorCode::GatewayUnavailable,
                    format!("failed to bind local gateway on port {port}: {error}"),
                ));
            }
        }
    }
}

fn prune_finished(running: &mut Option<RunningGateway>) {
    if running
        .as_ref()
        .is_some_and(|gateway| gateway.task.inner().is_finished())
    {
        running.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;
    use zenith_relay_core::{CandidateScope, LocalGatewayKey, ProviderSource, WireApi};

    async fn models_response(client: &reqwest::Client, url: &str, key: &str) -> String {
        client
            .get(url)
            .bearer_auth(key)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn stop_waits_until_the_same_port_can_be_rebound() {
        let runtime = Arc::new(
            GatewayRuntime::new(
                ProviderSource {
                    id: "source".into(),
                    name: "Test".into(),
                    base_url: "http://127.0.0.1:9/v1".into(),
                    api_key: "upstream".into(),
                    wire_api: WireApi::Responses,
                    models: vec!["gpt-test".into()],
                    proxy_url: None,
                },
                LocalGatewayKey {
                    id: "key".into(),
                    secret: "local".into(),
                },
                Arc::new(|_| {}),
            )
            .unwrap(),
        );
        let manager = GatewayManager::default();
        let first = manager.start(runtime.clone(), 0).await.unwrap();
        let mut stalled_client = TcpStream::connect(first).await.unwrap();
        stalled_client
            .write_all(
                b"POST /v1/responses HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\n\r\n{",
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        manager.stop().await;
        let second = manager.start(runtime, first.port()).await.unwrap();
        assert_eq!(first.port(), second.port());
        manager.stop().await;
        drop(stalled_client);
    }

    #[tokio::test]
    async fn restart_replaces_auth_and_model_registry_on_the_same_port() {
        let manager = GatewayManager::default();
        let first = manager
            .start(test_runtime("source_one", "model-one", "key-one"), 0)
            .await
            .unwrap();
        let client = reqwest::Client::new();
        let models_url = format!("http://{first}/v1/models");
        let first_models = models_response(&client, &models_url, "key-one").await;
        assert!(first_models.contains("model-one"));

        manager.stop().await;
        manager
            .start(
                test_runtime("source_two", "model-two", "key-two"),
                first.port(),
            )
            .await
            .unwrap();
        let restarted_client = reqwest::Client::new();
        assert_eq!(
            restarted_client
                .get(&models_url)
                .bearer_auth("key-one")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let second_models = models_response(&restarted_client, &models_url, "key-two").await;
        assert!(second_models.contains("model-two"));
        assert!(!second_models.contains("model-one"));
        manager.stop().await;
    }

    #[tokio::test]
    async fn replacement_does_not_wait_for_an_unfinished_request_body() {
        let manager = GatewayManager::default();
        let previous = test_runtime("source_one", "model-one", "key-one");
        let address = manager.start(previous.clone(), 0).await.unwrap();
        let mut stalled_client = TcpStream::connect(address).await.unwrap();
        stalled_client
            .write_all(
                b"POST /v1/responses HTTP/1.1\r\nHost: localhost\r\nContent-Length: 100\r\n\r\n{",
            )
            .await
            .unwrap();
        sleep(Duration::from_millis(20)).await;

        let next = test_runtime("source_two", "model-two", "key-two");
        assert!(timeout(
            Duration::from_millis(50),
            manager.replace_runtime(next.clone(), address.port()),
        )
        .await
        .expect("runtime replacement must not drain open requests"));
        assert_eq!(manager.address().await, Some(address));
        assert!(Arc::ptr_eq(&manager.runtime().await.unwrap(), &next));
        assert!(previous
            .candidate_runtime_order_for_key("local-source_one")
            .iter()
            .all(|candidate| !candidate.available));
        drop(stalled_client);
        manager.stop().await;
    }

    #[tokio::test]
    async fn replacement_updates_existing_keep_alive_connections() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

        async fn read_response(client: &mut BufReader<TcpStream>) -> (String, String) {
            let mut headers = String::new();
            let mut length = 0;
            loop {
                let mut line = String::new();
                assert_ne!(client.read_line(&mut line).await.unwrap(), 0);
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
                headers.push_str(&line);
            }
            let mut body = vec![0; length];
            client.read_exact(&mut body).await.unwrap();
            (headers, String::from_utf8(body).unwrap())
        }

        let manager = GatewayManager::default();
        let address = manager
            .start(test_runtime("source_one", "model-one", "key-one"), 0)
            .await
            .unwrap();
        let mut client = BufReader::new(TcpStream::connect(address).await.unwrap());
        let request = |key: &str| {
            format!(
                "GET /v1/models HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {key}\r\n\r\n"
            )
        };
        client
            .get_mut()
            .write_all(request("key-one").as_bytes())
            .await
            .unwrap();
        assert!(read_response(&mut client).await.1.contains("model-one"));

        assert!(
            manager
                .replace_runtime(
                    test_runtime("source_two", "model-two", "key-two"),
                    address.port(),
                )
                .await
        );
        client
            .get_mut()
            .write_all(request("key-one").as_bytes())
            .await
            .unwrap();
        assert!(read_response(&mut client)
            .await
            .0
            .starts_with("HTTP/1.1 401"));
        client
            .get_mut()
            .write_all(request("key-two").as_bytes())
            .await
            .unwrap();
        let (headers, models) = read_response(&mut client).await;
        assert!(headers.starts_with("HTTP/1.1 200"));
        assert!(models.contains("model-two"));
        assert!(!models.contains("model-one"));
        drop(client);
        manager.stop().await;
    }

    #[tokio::test]
    async fn replacement_preserves_an_already_dispatched_stream() {
        use axum::{body::Body, http::Response, routing::post, Router};
        use futures_util::{stream, StreamExt};
        use std::convert::Infallible;
        use tokio::sync::Notify;

        let finish = Arc::new(Notify::new());
        let upstream_finish = finish.clone();
        let upstream = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let upstream_address = upstream.local_addr().unwrap();
        let upstream_task = tauri::async_runtime::spawn(async move {
            let router = Router::new().route("/v1/responses", post(move || {
                let finish = upstream_finish.clone();
                async move {
                    let head = "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\",\"model\":\"model-one\",\"status\":\"in_progress\"}}\n\nevent: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\n\n";
                    let tail = "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"model\":\"model-one\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n";
                    let body = stream::once(async move { Ok::<_, Infallible>(head) }).chain(
                        stream::once(async move {
                            finish.notified().await;
                            Ok::<_, Infallible>(tail)
                        }),
                    );
                    Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(Body::from_stream(body))
                        .unwrap()
                }
            }));
            axum::serve(upstream, router).await.unwrap();
        });
        let manager = GatewayManager::default();
        let address = manager
            .start(
                test_runtime_at(
                    "source_one",
                    "model-one",
                    "key-one",
                    &format!("http://{upstream_address}/v1"),
                ),
                0,
            )
            .await
            .unwrap();
        let mut response = timeout(Duration::from_secs(2), reqwest::Client::new()
            .post(format!("http://{address}/v1/responses"))
            .bearer_auth("key-one")
            .json(&serde_json::json!({ "model": "model-one", "input": "synthetic", "stream": true }))
            .send()).await.unwrap().unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let first = timeout(Duration::from_secs(2), response.chunk())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!first.is_empty());
        assert!(
            manager
                .replace_runtime(
                    test_runtime("source_two", "model-two", "key-two"),
                    address.port(),
                )
                .await
        );
        finish.notify_one();
        let remaining = timeout(Duration::from_secs(2), response.text())
            .await
            .unwrap()
            .unwrap();
        assert!(remaining.contains("response.completed"));
        manager.stop().await;
        upstream_task.abort();
        let _ = upstream_task.await;
    }

    #[tokio::test]
    async fn replacement_requires_a_running_listener_on_the_requested_port() {
        let manager = GatewayManager::default();
        let previous = test_runtime("source_one", "model-one", "key-one");
        let next = test_runtime("source_two", "model-two", "key-two");
        assert!(!manager.replace_runtime(next.clone(), 0).await);
        let address = manager.start(previous.clone(), 0).await.unwrap();
        let other_port = if address.port() == u16::MAX {
            1
        } else {
            address.port() + 1
        };
        assert!(!manager.replace_runtime(next, other_port).await);
        assert!(Arc::ptr_eq(&manager.runtime().await.unwrap(), &previous));
        manager.stop().await;
    }

    #[tokio::test]
    async fn candidate_availability_updates_without_rebinding_the_listener() {
        let manager = GatewayManager::default();
        let runtime = test_runtime("source", "model", "key");
        let address = manager.start(runtime.clone(), 0).await.unwrap();
        let models_url = format!("http://{address}/v1/models");
        let client = reqwest::Client::new();
        assert!(models_response(&client, &models_url, "key")
            .await
            .contains("model"));

        let running_runtime = manager.runtime().await.unwrap();
        assert!(Arc::ptr_eq(&running_runtime, &runtime));
        assert!(running_runtime.update_source_policy(
            "source",
            zenith_relay_core::RuntimeCandidatePolicy {
                enabled: false,
                draining: false,
                priority: 0,
                weight: 1,
                allowed_models: vec![],
                excluded_models: vec![],
            },
            0,
        ));
        assert_eq!(manager.address().await, Some(address));
        assert!(!models_response(&client, &models_url, "key")
            .await
            .contains("model"));
        manager.stop().await;
    }

    #[tokio::test]
    async fn start_retries_a_port_released_by_a_previous_listener() {
        let manager = GatewayManager::default();
        let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = occupied.local_addr().unwrap().port();
        let release = tauri::async_runtime::spawn(async move {
            sleep(Duration::from_millis(20)).await;
            drop(occupied);
        });

        let address = manager
            .start(test_runtime("source", "model", "key"), port)
            .await
            .unwrap();

        assert_eq!(address.port(), port);
        release.await.unwrap();
        manager.stop().await;
    }

    #[tokio::test]
    async fn key_scope_updates_without_rebinding_the_listener() {
        let manager = GatewayManager::default();
        let runtime = test_runtime("source", "model", "key");
        let address = manager.start(runtime.clone(), 0).await.unwrap();
        let models_url = format!("http://{address}/v1/models");
        let client = reqwest::Client::new();
        assert!(models_response(&client, &models_url, "key")
            .await
            .contains("model"));

        assert!(runtime.update_key_scope(
            "local-source",
            CandidateScope {
                source_ids: Some(Default::default()),
                account_ids: Some(Default::default()),
                model_rules: Default::default(),
            },
        ));
        assert_eq!(manager.address().await, Some(address));
        assert!(!models_response(&client, &models_url, "key")
            .await
            .contains("model"));
        manager.stop().await;
    }

    #[tokio::test]
    async fn finished_gateway_task_does_not_block_a_fresh_start() {
        let manager = GatewayManager::default();
        let runtime = test_runtime("source", "model", "key");
        let (shutdown, _receiver) = oneshot::channel();
        let task = tauri::async_runtime::spawn(async {});
        timeout(Duration::from_secs(1), async {
            while !task.inner().is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        *manager.running.lock().await = Some(RunningGateway {
            address: "127.0.0.1:9".parse().unwrap(),
            routes: watch::channel(GatewayRoutes::new(runtime.clone())).0,
            shutdown,
            task,
        });

        assert_eq!(manager.address().await, None);
        let address = manager.start(runtime, 0).await.unwrap();
        assert_ne!(address.port(), 9);
        manager.stop().await;
    }

    fn test_runtime(source_id: &str, model: &str, key: &str) -> Arc<GatewayRuntime> {
        test_runtime_at(source_id, model, key, "http://127.0.0.1:9/v1")
    }

    fn test_runtime_at(
        source_id: &str,
        model: &str,
        key: &str,
        base_url: &str,
    ) -> Arc<GatewayRuntime> {
        Arc::new(
            GatewayRuntime::new(
                ProviderSource {
                    id: source_id.into(),
                    name: "Test".into(),
                    base_url: base_url.into(),
                    api_key: "upstream".into(),
                    wire_api: WireApi::Responses,
                    models: vec![model.into()],
                    proxy_url: None,
                },
                LocalGatewayKey {
                    id: format!("local-{source_id}"),
                    secret: key.into(),
                },
                Arc::new(|_| {}),
            )
            .unwrap(),
        )
    }
}

use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    net::TcpListener,
    sync::{oneshot, Mutex},
    time::{timeout, Duration},
};
use zenith_relay_core::GatewayRuntime;

#[cfg(not(test))]
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(100);

struct RunningGateway {
    address: SocketAddr,
    shutdown: oneshot::Sender<()>,
    task: tauri::async_runtime::JoinHandle<()>,
}

#[derive(Default)]
pub struct GatewayManager {
    running: Mutex<Option<RunningGateway>>,
}

impl GatewayManager {
    pub async fn start(&self, runtime: Arc<GatewayRuntime>, port: u16) -> Result<SocketAddr> {
        let mut running = self.running.lock().await;
        if let Some(current) = running.as_ref() {
            return Ok(current.address);
        }
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|error| {
                LocalPoolError::new(
                    ErrorCode::GatewayUnavailable,
                    format!("failed to bind local gateway on port {port}: {error}"),
                )
            })?;
        let address = listener.local_addr().map_err(|error| {
            LocalPoolError::new(ErrorCode::GatewayUnavailable, error.to_string())
        })?;
        let (shutdown, receiver) = oneshot::channel();
        let task = tauri::async_runtime::spawn(async move {
            let _ = axum::serve(listener, zenith_relay_core::gateway::router(runtime))
                .with_graceful_shutdown(async move {
                    let _ = receiver.await;
                })
                .await;
        });
        *running = Some(RunningGateway {
            address,
            shutdown,
            task,
        });
        Ok(address)
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
        self.running
            .lock()
            .await
            .as_ref()
            .map(|running| running.address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;
    use zenith_relay_core::{LocalGatewayKey, ProviderSource, WireApi};

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
        let first_models = client
            .get(&models_url)
            .bearer_auth("key-one")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(first_models.contains("model-one"));

        manager.stop().await;
        manager
            .start(
                test_runtime("source_two", "model-two", "key-two"),
                first.port(),
            )
            .await
            .unwrap();
        assert_eq!(
            client
                .get(&models_url)
                .bearer_auth("key-one")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let second_models = client
            .get(&models_url)
            .bearer_auth("key-two")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(second_models.contains("model-two"));
        assert!(!second_models.contains("model-one"));
        manager.stop().await;
    }

    fn test_runtime(source_id: &str, model: &str, key: &str) -> Arc<GatewayRuntime> {
        Arc::new(
            GatewayRuntime::new(
                ProviderSource {
                    id: source_id.into(),
                    name: "Test".into(),
                    base_url: "http://127.0.0.1:9/v1".into(),
                    api_key: "upstream".into(),
                    wire_api: WireApi::Responses,
                    models: vec![model.into()],
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

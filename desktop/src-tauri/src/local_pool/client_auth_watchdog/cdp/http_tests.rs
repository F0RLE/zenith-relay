use super::{http_client, query_targets};
use axum::{Json, Router, response::Redirect, routing::get};
use serde_json::json;
use tokio::{net::TcpListener, task::JoinHandle};

#[tokio::test]
async fn discovery_ignores_system_proxy() {
    if crate::tests::run_isolated_proxy_test(concat!(
        module_path!(),
        "::discovery_ignores_system_proxy"
    ))
    .await
    {
        return;
    }
    let (port, server) = serve(targets_router()).await;
    let targets = query_targets(&http_client().unwrap(), port).await;
    server.abort();
    let _ = server.await;
    assert_eq!(
        targets.len(),
        1,
        "CDP discovery must bypass inherited proxies"
    );
    assert!(targets[0].is_codex_app_page());
}

#[tokio::test]
async fn discovery_does_not_follow_redirects() {
    if crate::tests::run_isolated_proxy_test(concat!(
        module_path!(),
        "::discovery_does_not_follow_redirects"
    ))
    .await
    {
        return;
    }
    let client = http_client().unwrap();
    let (destination_port, destination) = serve(targets_router()).await;
    assert_eq!(query_targets(&client, destination_port).await.len(), 1);
    let location = format!("http://127.0.0.1:{destination_port}/json/list");
    let app = Router::new().route(
        "/json/list",
        get(move || {
            let location = location.clone();
            async move { Redirect::temporary(&location) }
        }),
    );
    let (port, redirect) = serve(app).await;
    let targets = query_targets(&client, port).await;
    for server in [redirect, destination] {
        server.abort();
        let _ = server.await;
    }
    assert!(
        targets.is_empty(),
        "CDP discovery must not leave its original endpoint"
    );
}

fn targets_router() -> Router {
    Router::new().route(
        "/json/list",
        get(|| async {
            Json(json!([{
                "type": "page",
                "url": "app://-/index.html",
                "webSocketDebuggerUrl": "ws://127.0.0.1:1/devtools/page/synthetic",
            }]))
        }),
    )
}

async fn serve(app: Router) -> (u16, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (port, server)
}

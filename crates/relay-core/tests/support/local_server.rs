use axum::Router;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub(super) struct TestServer {
    pub(super) base_url: String,
    task: JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub(super) async fn spawn(app: Router) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    TestServer {
        base_url: format!("http://{address}"),
        task,
    }
}

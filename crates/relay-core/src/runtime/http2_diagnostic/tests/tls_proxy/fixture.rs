use super::*;
use crate::runtime::http2_diagnostic::fixture::serve;
use crate::runtime::support::runtime_client_builder;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub(super) struct TlsPeer {
    pub(super) address: std::net::SocketAddr,
    pub(super) url: String,
    pub(super) certificate: reqwest::Certificate,
    pub(super) requests: Arc<AtomicUsize>,
    pub(super) connections: Arc<AtomicUsize>,
}

impl TlsPeer {
    pub(super) async fn start(tasks: &mut JoinSet<()>, reply: Reply) -> Self {
        let certificate = rcgen::generate_simple_self_signed(vec!["127.0.0.1".into()]).unwrap();
        let root = reqwest::Certificate::from_der(certificate.cert.der()).unwrap();
        let key =
            rustls::pki_types::PrivatePkcs8KeyDer::from(certificate.signing_key.serialize_der());
        let mut tls = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::aws_lc_rs::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![certificate.cert.der().clone()], key.into())
        .unwrap();
        tls.alpn_protocols = vec![b"h2".to_vec()];
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let connections = Arc::new(AtomicUsize::new(0));
        let observed_requests = requests.clone();
        let observed_connections = connections.clone();
        tasks.spawn(async move {
            let mut children = JoinSet::new();
            for _ in 0..8 {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let acceptor = acceptor.clone();
                let requests = observed_requests.clone();
                let connections = observed_connections.clone();
                children.spawn(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(5), async move {
                        let Ok(socket) = acceptor.accept(socket).await else {
                            return;
                        };
                        assert_eq!(socket.get_ref().1.alpn_protocol(), Some(b"h2".as_slice()));
                        connections.fetch_add(1, Ordering::SeqCst);
                        serve(socket, requests, reply).await;
                    })
                    .await;
                });
            }
            while let Some(result) = children.join_next().await {
                result.unwrap();
            }
        });
        Self {
            address,
            url: format!("https://{address}/v1"),
            certificate: root,
            requests,
            connections,
        }
    }

    pub(super) fn client(
        &self,
        proxy: Option<&crate::proxy::ProxyConfig>,
        default_retries: bool,
    ) -> reqwest::Client {
        let builder = if default_retries {
            let builder = reqwest::Client::builder().no_proxy();
            match proxy {
                Some(proxy) => proxy.apply(builder),
                None => builder,
            }
        } else if proxy.is_some() {
            runtime_client_builder(proxy)
        } else {
            runtime_client_builder(None).no_proxy()
        };
        builder
            .add_root_certificate(self.certificate.clone())
            .https_only(true)
            .build()
            .unwrap()
    }
}

pub(super) struct ConnectProxy {
    pub(super) url: String,
    pub(super) tunnels: Arc<AtomicUsize>,
}

impl ConnectProxy {
    pub(super) async fn start(tasks: &mut JoinSet<()>, target: std::net::SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let tunnels = Arc::new(AtomicUsize::new(0));
        let observed = tunnels.clone();
        tasks.spawn(async move {
            let mut children = JoinSet::new();
            for _ in 0..8 {
                let Ok((mut downstream, _)) = listener.accept().await else {
                    break;
                };
                let tunnels = observed.clone();
                children.spawn(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(5), async move {
                        let mut headers = Vec::new();
                        while !headers.ends_with(b"\r\n\r\n") {
                            assert!(headers.len() < 4096, "CONNECT header limit exceeded");
                            let Ok(byte) = downstream.read_u8().await else {
                                return;
                            };
                            headers.push(byte);
                        }
                        assert!(
                            headers
                                .starts_with(format!("CONNECT {target} HTTP/1.1\r\n").as_bytes())
                        );
                        let mut upstream = TcpStream::connect(target).await.unwrap();
                        tunnels.fetch_add(1, Ordering::SeqCst);
                        downstream
                            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                            .await
                            .unwrap();
                        let _ = tokio::io::copy_bidirectional(&mut downstream, &mut upstream).await;
                    })
                    .await;
                });
            }
            while let Some(result) = children.join_next().await {
                result.unwrap();
            }
        });
        Self { url, tunnels }
    }

    pub(super) fn config(&self) -> crate::proxy::ProxyConfig {
        crate::proxy::ProxyConfig::parse(&self.url).unwrap()
    }
}

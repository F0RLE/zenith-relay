use crate::local_pool::error::{ErrorCode, LocalPoolError};
use base64::{engine::general_purpose::STANDARD, Engine};
use std::{fmt, io, net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::oneshot,
};
use url::Url;
use zenith_relay_core::normalize_proxy_url;

const HEADER_LIMIT: usize = 16 * 1024;
const HTTPS_PROXY_MESSAGE: &str =
    "The sign-in window cannot use an HTTPS proxy. Use an HTTP proxy for this account.";

enum SignInProxyPlan {
    Direct,
    Http(Url),
    Bridge {
        host: String,
        port: u16,
        username: String,
        password: String,
    },
}

impl fmt::Debug for SignInProxyPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Direct => formatter.write_str("Direct"),
            Self::Http(url) => formatter.debug_tuple("Http").field(&url.as_str()).finish(),
            Self::Bridge { host, port, .. } => formatter
                .debug_struct("Bridge")
                .field("host", host)
                .field("port", port)
                .field("credentials", &"[redacted]")
                .finish(),
        }
    }
}

pub(super) struct ProxyBridge {
    shutdown: Option<oneshot::Sender<()>>,
    local_address: SocketAddr,
}

impl fmt::Debug for ProxyBridge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProxyBridge([redacted])")
    }
}

impl Drop for ProxyBridge {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

pub(super) async fn webview_proxy(
    proxy_url: Option<&str>,
) -> Result<(Option<Url>, Option<ProxyBridge>), LocalPoolError> {
    match sign_in_proxy_plan(proxy_url)? {
        SignInProxyPlan::Direct => Ok((None, None)),
        SignInProxyPlan::Http(url) => Ok((Some(url), None)),
        SignInProxyPlan::Bridge {
            host,
            port,
            username,
            password,
        } => {
            let bridge = ProxyBridge::start(&host, port, &username, &password).await?;
            let url = http_endpoint_url(
                &bridge.local_address.ip().to_string(),
                bridge.local_address.port(),
            )?;
            Ok((Some(url), Some(bridge)))
        }
    }
}

fn sign_in_proxy_plan(proxy_url: Option<&str>) -> Result<SignInProxyPlan, LocalPoolError> {
    let Some(proxy_url) = proxy_url.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(SignInProxyPlan::Direct);
    };
    let normalized = normalize_proxy_url(proxy_url).map_err(|_| invalid_proxy())?;
    let url = Url::parse(&normalized).map_err(|_| invalid_proxy())?;
    if url.scheme() == "https" {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            HTTPS_PROXY_MESSAGE,
        ));
    }
    if url.scheme() != "http" {
        return Err(invalid_proxy());
    }
    let host = url
        .host_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(invalid_proxy)?
        .to_string();
    let port = url.port().ok_or_else(invalid_proxy)?;
    let username = url.username().to_string();
    let password = url.password().map(str::to_string);
    if username.is_empty() && password.is_none() {
        return Ok(SignInProxyPlan::Http(http_endpoint_url(&host, port)?));
    }
    let Some(password) = password.filter(|value| !value.is_empty()) else {
        return Err(invalid_proxy());
    };
    if username.is_empty() || username.contains(['\r', '\n']) || password.contains(['\r', '\n']) {
        return Err(invalid_proxy());
    }
    Ok(SignInProxyPlan::Bridge {
        host,
        port,
        username,
        password,
    })
}

fn http_endpoint_url(host: &str, port: u16) -> Result<Url, LocalPoolError> {
    let endpoint = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    Url::parse(&format!("http://{endpoint}/")).map_err(|_| invalid_proxy())
}

fn invalid_proxy() -> LocalPoolError {
    LocalPoolError::new(ErrorCode::InvalidState, "stored proxy URL is invalid")
}

impl ProxyBridge {
    async fn start(
        host: &str,
        port: u16,
        username: &str,
        password: &str,
    ) -> Result<Self, LocalPoolError> {
        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .map_err(|_| super::window_prepare_error())?;
        let local_address = listener
            .local_addr()
            .map_err(|_| super::window_prepare_error())?;
        let (shutdown, receiver) = oneshot::channel();
        let upstream_host = host.to_string();
        let authorization = STANDARD.encode(format!("{username}:{password}"));
        tokio::spawn(async move {
            run_proxy_bridge(listener, receiver, upstream_host, port, authorization).await;
        });
        Ok(Self {
            shutdown: Some(shutdown),
            local_address,
        })
    }
}

async fn run_proxy_bridge(
    listener: tokio::net::TcpListener,
    mut shutdown: oneshot::Receiver<()>,
    host: String,
    port: u16,
    authorization: String,
) {
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else { break };
                let host = host.clone();
                let authorization = authorization.clone();
                tokio::spawn(async move {
                    let _ = handle_proxy_client(stream, &host, port, &authorization).await;
                });
            }
        }
    }
}

async fn handle_proxy_client(
    mut client: TcpStream,
    host: &str,
    port: u16,
    authorization: &str,
) -> io::Result<()> {
    let (head, rest) = read_http_head(&mut client, HEADER_LIMIT).await?;
    let request = rewrite_proxy_request(&head, authorization)?;
    let connect = is_connect_request(&head);
    let mut upstream =
        match tokio::time::timeout(Duration::from_secs(20), TcpStream::connect((host, port))).await
        {
            Ok(Ok(stream)) => stream,
            _ => {
                write_gateway_error(&mut client).await?;
                return Ok(());
            }
        };
    upstream.write_all(&request).await?;
    if !rest.is_empty() {
        upstream.write_all(&rest).await?;
    }
    if connect {
        let (status, extra) = read_http_head(&mut upstream, HEADER_LIMIT).await?;
        if status_code(&status) != Some(200) {
            write_gateway_error(&mut client).await?;
            return Ok(());
        }
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        if !extra.is_empty() {
            client.write_all(&extra).await?;
        }
    }
    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
    Ok(())
}

fn rewrite_proxy_request(head: &[u8], authorization: &str) -> io::Result<Vec<u8>> {
    let text = std::str::from_utf8(head)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "proxy header is invalid"))?;
    let mut lines = text.split("\r\n");
    let request = lines
        .next()
        .filter(|line| !line.is_empty())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "proxy request is empty"))?;
    let mut output = Vec::with_capacity(head.len() + authorization.len() + 32);
    output.extend_from_slice(request.as_bytes());
    output.extend_from_slice(b"\r\nProxy-Authorization: Basic ");
    output.extend_from_slice(authorization.as_bytes());
    output.extend_from_slice(b"\r\n");
    for line in lines {
        if line.is_empty() {
            break;
        }
        if !line
            .to_ascii_lowercase()
            .starts_with("proxy-authorization:")
        {
            output.extend_from_slice(line.as_bytes());
            output.extend_from_slice(b"\r\n");
        }
    }
    output.extend_from_slice(b"\r\n");
    Ok(output)
}

fn is_connect_request(head: &[u8]) -> bool {
    head.len() >= 8 && head[..8].eq_ignore_ascii_case(b"CONNECT ")
}

fn status_code(head: &[u8]) -> Option<u16> {
    let text = std::str::from_utf8(head).ok()?;
    let line = text.split("\r\n").next()?;
    line.split_whitespace().nth(1)?.parse().ok()
}

async fn write_gateway_error(stream: &mut TcpStream) -> io::Result<()> {
    stream
        .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .await
}

async fn read_http_head(stream: &mut TcpStream, limit: usize) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "proxy header ended",
            ));
        }
        if buffer.len().saturating_add(read) > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "proxy header is too large",
            ));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(end) = header_end(&buffer) {
            let rest = buffer.split_off(end);
            return Ok((buffer, rest));
        }
    }
}

fn header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|index| index + 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_plan_keeps_credentials_out_of_the_webview_url_and_errors() {
        assert!(matches!(
            sign_in_proxy_plan(None).unwrap(),
            SignInProxyPlan::Direct
        ));

        let direct = sign_in_proxy_plan(Some("http://proxy.example:8080")).unwrap();
        match direct {
            SignInProxyPlan::Http(url) => {
                assert_eq!(url.as_str(), "http://proxy.example:8080/");
                assert!(url.username().is_empty());
                assert!(url.password().is_none());
            }
            other => panic!("expected plain http proxy, got {other:?}"),
        }

        let bridged =
            sign_in_proxy_plan(Some("proxy.example:8080:login-name:secret-value")).unwrap();
        let rendered = format!("{bridged:?}");
        assert!(!rendered.contains("login-name"));
        assert!(!rendered.contains("secret-value"));
        match bridged {
            SignInProxyPlan::Bridge {
                host,
                port,
                username,
                password,
            } => {
                assert_eq!(host, "proxy.example");
                assert_eq!(port, 8080);
                assert_eq!(username, "login-name");
                assert_eq!(password, "secret-value");
            }
            other => panic!("expected authenticated proxy, got {other:?}"),
        }

        let error = sign_in_proxy_plan(Some("https://login-name:secret-value@proxy.example:8443"))
            .unwrap_err();
        let rendered = format!("{error} {error:?}");
        assert!(rendered.contains("HTTPS"));
        assert!(!rendered.contains("login-name"));
        assert!(!rendered.contains("secret-value"));
        assert!(sign_in_proxy_plan(Some("http://login-name@proxy.example:8080")).is_err());
    }

    #[tokio::test]
    async fn authenticated_bridge_adds_proxy_authorization_and_hides_upstream_failures() {
        let (sender, receiver) = oneshot::channel();
        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (head, rest) = read_http_head(&mut socket, HEADER_LIMIT).await.unwrap();
            let _ = sender.send(String::from_utf8(head).unwrap());
            socket
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .unwrap();
            if rest.is_empty() {
                let mut byte = [0_u8; 1];
                socket.read_exact(&mut byte).await.unwrap();
                socket.write_all(&byte).await.unwrap();
            } else {
                socket.write_all(&rest).await.unwrap();
            }
        });

        let bridge = ProxyBridge::start(&address.ip().to_string(), address.port(), "user", "pass")
            .await
            .unwrap();
        assert!(!format!("{bridge:?}").contains("pass"));
        let mut client = TcpStream::connect(bridge.local_address).await.unwrap();
        client
            .write_all(b"CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\nZ")
            .await
            .unwrap();
        let (response, extra) = read_http_head(&mut client, HEADER_LIMIT).await.unwrap();
        assert_eq!(status_code(&response), Some(200));
        let echoed = if extra.is_empty() {
            let mut byte = [0_u8; 1];
            client.read_exact(&mut byte).await.unwrap();
            byte[0]
        } else {
            extra[0]
        };
        assert_eq!(echoed, b'Z');
        let seen = receiver.await.unwrap();
        let expected = STANDARD.encode("user:pass");
        assert!(seen.starts_with("CONNECT example.test:443 "));
        assert!(seen.contains(&format!("Proxy-Authorization: Basic {expected}\r\n")));
        drop(bridge);

        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = read_http_head(&mut socket, HEADER_LIMIT).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 12\r\n\r\nsecret-realm")
                .await
                .unwrap();
        });
        let bridge = ProxyBridge::start(&address.ip().to_string(), address.port(), "user", "pass")
            .await
            .unwrap();
        let mut client = TcpStream::connect(bridge.local_address).await.unwrap();
        client
            .write_all(b"CONNECT example.test:443 HTTP/1.1\r\nHost: example.test:443\r\n\r\n")
            .await
            .unwrap();
        let (response, extra) = read_http_head(&mut client, HEADER_LIMIT).await.unwrap();
        let rendered = String::from_utf8_lossy(&response);
        assert!(rendered.contains("502"));
        assert!(!rendered.contains("secret-realm"));
        assert!(!String::from_utf8_lossy(&extra).contains("secret-realm"));
    }
}

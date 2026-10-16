use super::super::import_session::SecretBackend;
use super::super::oauth::{OAuthPendingSession, CODEX_OAUTH_CALLBACK_PORTS};
use super::{
    OAuthFlowError, OAuthFlowErrorCode, OAuthFlowEventSink, OAuthFlowInner, OAuthFlowStatus,
    PendingSnapshot, CALLBACK_PATH,
};
use std::io;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use url::Url;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CallbackLanguage {
    English,
    Russian,
}

macro_rules! callback_success_html {
    ($language:literal, $heading:literal, $message:literal) => {
        concat!(
            "<!doctype html><html lang=\"",
            $language,
            "\"><meta charset=\"utf-8\"><meta name=\"color-scheme\" content=\"light dark\"><title>Zenith Relay</title><style>",
            "body{min-height:100vh;display:grid;place-items:center;box-sizing:border-box;margin:0;padding:24px;font:15px system-ui,sans-serif;background:Canvas;color:CanvasText;user-select:none;-webkit-user-select:none}",
            "main{width:min(100%,420px);box-sizing:border-box;padding:32px;text-align:center}",
            "h1{margin:0 0 8px;font-size:24px}",
            "p{margin:0;color:GrayText;line-height:1.5}",
            "</style><body><main><h1>",
            $heading,
            "</h1><p>",
            $message,
            "</p></main></body></html>",
        )
    };
}

const CALLBACK_SUCCESS_HTML_EN: &str =
    callback_success_html!("en", "Account connected", "You can close this window now.");
const CALLBACK_SUCCESS_HTML_RU: &str =
    callback_success_html!("ru", "Аккаунт подключён", "Теперь это окно можно закрыть.");

pub(super) fn callback_success_html(language: CallbackLanguage) -> &'static str {
    match language {
        CallbackLanguage::English => CALLBACK_SUCCESS_HTML_EN,
        CallbackLanguage::Russian => CALLBACK_SUCCESS_HTML_RU,
    }
}

const MAX_REQUEST_LINE_BYTES: usize = 8 * 1024;
pub(super) const MAX_REQUEST_HEADER_BYTES: usize = 16 * 1024;
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) async fn run_listener<B, E>(
    inner: Arc<OAuthFlowInner<B, E>>,
    listener: TcpListener,
    snapshot: PendingSnapshot,
    started_at_ms: u64,
    mut shutdown: oneshot::Receiver<()>,
) where
    B: SecretBackend + Send + Sync + 'static,
    E: OAuthFlowEventSink,
{
    let remaining_ms = snapshot
        .pending
        .expires_at_ms()
        .saturating_sub(started_at_ms);
    let expiry = tokio::time::sleep(Duration::from_millis(remaining_ms));
    tokio::pin!(expiry);
    loop {
        tokio::select! {
            _ = &mut shutdown => break,
            _ = &mut expiry => {
                let _ = inner.cleanup(&snapshot.login_id);
                inner.emit(&snapshot.login_id, OAuthFlowStatus::Expired);
                break;
            }
            accepted = listener.accept() => {
                let Ok((stream, _)) = accepted else {
                    inner.emit(&snapshot.login_id, OAuthFlowStatus::Failed);
                    break;
                };
                match process_request(&inner, &snapshot, stream).await {
                    RequestOutcome::Accepted => break,
                    RequestOutcome::Rejected => {
                        inner.emit(&snapshot.login_id, OAuthFlowStatus::CallbackRejected);
                    }
                    RequestOutcome::Failed => {
                        inner.emit(&snapshot.login_id, OAuthFlowStatus::Failed);
                        break;
                    }
                }
            }
        }
    }
}

enum RequestOutcome {
    Accepted,
    Rejected,
    Failed,
}

async fn process_request<B, E>(
    inner: &OAuthFlowInner<B, E>,
    snapshot: &PendingSnapshot,
    mut stream: TcpStream,
) -> RequestOutcome
where
    B: SecretBackend,
    E: OAuthFlowEventSink,
{
    let request = match read_request(&mut stream).await {
        Ok(request) => request,
        Err(RequestReadError::TooLarge) => {
            let _ = write_response(&mut stream, 413, "OAuth callback request is too large.").await;
            return RequestOutcome::Rejected;
        }
        Err(RequestReadError::Invalid) => {
            let _ = write_response(&mut stream, 400, "Invalid OAuth callback request.").await;
            return RequestOutcome::Rejected;
        }
        Err(RequestReadError::Io) => return RequestOutcome::Rejected,
    };
    let Ok(callback_url) = callback_url(&snapshot.pending, &request.target) else {
        let _ = write_response(&mut stream, 400, "Invalid OAuth callback request.").await;
        return RequestOutcome::Rejected;
    };
    match inner.accept_callback(&snapshot.login_id, &callback_url) {
        Ok(()) => {
            let _ = write_callback_success(&mut stream, request.language).await;
            RequestOutcome::Accepted
        }
        Err(error) if error.code == OAuthFlowErrorCode::CallbackInvalid => {
            let _ = write_response(&mut stream, 400, "Invalid OAuth callback.").await;
            RequestOutcome::Rejected
        }
        Err(_) => {
            let _ = write_response(&mut stream, 500, "OAuth callback could not be saved.").await;
            RequestOutcome::Failed
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequestReadError {
    Invalid,
    Io,
    TooLarge,
}

struct CallbackRequest {
    target: String,
    language: CallbackLanguage,
}

async fn read_request(stream: &mut TcpStream) -> Result<CallbackRequest, RequestReadError> {
    let read = async {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let count = stream
                .read(&mut buffer)
                .await
                .map_err(|_| RequestReadError::Io)?;
            if count == 0 {
                return Err(RequestReadError::Invalid);
            }
            request.extend_from_slice(&buffer[..count]);
            if request.len() > MAX_REQUEST_HEADER_BYTES {
                return Err(RequestReadError::TooLarge);
            }
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }
        let header = std::str::from_utf8(&request).map_err(|_| RequestReadError::Invalid)?;
        let request_line = header
            .split("\r\n")
            .next()
            .ok_or(RequestReadError::Invalid)?;
        if request_line.len() > MAX_REQUEST_LINE_BYTES {
            return Err(RequestReadError::TooLarge);
        }
        let mut parts = request_line.split(' ');
        let (Some("GET"), Some(target), Some("HTTP/1.1"), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(RequestReadError::Invalid);
        };
        if !target.starts_with('/') || target.bytes().any(|byte| byte.is_ascii_control()) {
            return Err(RequestReadError::Invalid);
        }
        Ok(CallbackRequest {
            target: target.to_string(),
            language: callback_language(header),
        })
    };
    tokio::time::timeout(REQUEST_READ_TIMEOUT, read)
        .await
        .map_err(|_| RequestReadError::Io)?
}

pub(super) fn callback_language(headers: &str) -> CallbackLanguage {
    let Some(accept_language_header) = headers.lines().find_map(|line| {
        line.split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("accept-language"))
            .map(|(_, header_value)| header_value)
    }) else {
        return CallbackLanguage::English;
    };

    for preference in accept_language_header.split(',') {
        match preference
            .split(';')
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            language if language == "ru" || language.starts_with("ru-") => {
                return CallbackLanguage::Russian;
            }
            language if language == "en" || language.starts_with("en-") => {
                return CallbackLanguage::English;
            }
            _ => {}
        }
    }
    CallbackLanguage::English
}

fn callback_url(pending: &OAuthPendingSession, target: &str) -> Result<String, OAuthFlowError> {
    let request_url = Url::parse(&format!("http://localhost{target}")).map_err(|_| {
        OAuthFlowError::new(
            OAuthFlowErrorCode::CallbackInvalid,
            "OAuth callback request is invalid",
        )
    })?;
    if request_url.path() != CALLBACK_PATH || request_url.fragment().is_some() {
        return Err(OAuthFlowError::new(
            OAuthFlowErrorCode::CallbackInvalid,
            "OAuth callback path is invalid",
        ));
    }
    let mut callback_url = Url::parse(pending.redirect_uri()).map_err(|_| {
        OAuthFlowError::new(
            OAuthFlowErrorCode::RecoveryRequired,
            "OAuth pending redirect requires recovery",
        )
    })?;
    callback_url.set_query(request_url.query());
    Ok(callback_url.to_string())
}

async fn write_response(
    stream: &mut TcpStream,
    status: u16,
    response_body: &str,
) -> io::Result<()> {
    write_http_response(stream, status, "text/plain; charset=utf-8", response_body).await
}

async fn write_callback_success(
    stream: &mut TcpStream,
    language: CallbackLanguage,
) -> io::Result<()> {
    write_http_response(
        stream,
        200,
        "text/html; charset=utf-8",
        callback_success_html(language),
    )
    .await
}

async fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    response_body: &str,
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        413 => "Content Too Large",
        _ => "Internal Server Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
        response_body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

pub(super) async fn bind_callback_listener() -> Result<TcpListener, OAuthFlowError> {
    let mut listener_error = false;
    for port in CODEX_OAUTH_CALLBACK_PORTS {
        match TcpListener::bind(("127.0.0.1", port)).await {
            Ok(listener) => return Ok(listener),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {}
            Err(_) => listener_error = true,
        }
    }
    Err(OAuthFlowError::new(
        if listener_error {
            OAuthFlowErrorCode::ListenerUnavailable
        } else {
            OAuthFlowErrorCode::CallbackPortUnavailable
        },
        "OAuth callback ports 1455 and 1457 are unavailable",
    ))
}

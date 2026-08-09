use super::import_session::SecretBackend;
use super::oauth::{
    CodexOAuthClient, OAuthCallback, OAuthPendingSession, CODEX_OAUTH_CALLBACK_PORTS,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use url::Url;
use uuid::Uuid;

const SNAPSHOT_VERSION: u32 = 1;
const AUTHORIZATION_ENDPOINT: &str = "https://auth.openai.com/oauth/authorize";
const CALLBACK_PATH: &str = "/auth/callback";
const CALLBACK_SUCCESS_HTML: &str = r#"<!doctype html><html lang="en"><meta charset="utf-8"><meta name="color-scheme" content="light dark"><title>Zenith Relay</title><style>body{min-height:100vh;display:grid;place-items:center;margin:0;font:15px system-ui,sans-serif;background:Canvas;color:CanvasText}main{max-width:420px;padding:32px}h1{margin:0 0 8px;font-size:24px}p{margin:0;color:GrayText;line-height:1.5}</style><body><main><h1>Account connected</h1><p>You can close this tab and return to Zenith Relay.</p></main><script>window.close()</script></body></html>"#;
const MAX_SNAPSHOT_BYTES: u64 = 64 * 1024;
const MAX_REQUEST_LINE_BYTES: usize = 8 * 1024;
const MAX_REQUEST_HEADER_BYTES: usize = 16 * 1024;
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);

pub trait OAuthFlowEventSink: Send + Sync + 'static {
    fn emit(&self, event: OAuthFlowEvent);
}

impl<F> OAuthFlowEventSink for F
where
    F: Fn(OAuthFlowEvent) + Send + Sync + 'static,
{
    fn emit(&self, event: OAuthFlowEvent) {
        self(event);
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthFlowEvent {
    pub login_id: String,
    pub status: OAuthFlowStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthFlowStatus {
    Pending,
    CallbackReceived,
    CallbackRejected,
    Canceled,
    Completed,
    Expired,
    Failed,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthFlowStart {
    pub login_id: String,
    pub authorization_url: String,
    pub redirect_uri: String,
    pub expires_at_ms: u64,
    pub status: OAuthFlowStatus,
}

impl fmt::Debug for OAuthFlowStart {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthFlowStart")
            .field("login_id", &self.login_id)
            .field("authorization_url", &"[redacted]")
            .field("redirect_uri", &self.redirect_uri)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("status", &self.status)
            .finish()
    }
}

pub struct OAuthExchangeMaterial {
    pending: OAuthPendingSession,
    callback: OAuthCallback,
}

impl OAuthExchangeMaterial {
    pub fn into_parts(self) -> (OAuthPendingSession, OAuthCallback) {
        (self.pending, self.callback)
    }
}

impl fmt::Debug for OAuthExchangeMaterial {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthExchangeMaterial")
            .field("pending", &"[redacted]")
            .field("callback", &"[redacted]")
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthFlowErrorCode {
    CallbackAlreadyReceived,
    CallbackInvalid,
    CallbackPortUnavailable,
    CleanupIncomplete,
    Expired,
    InvalidLoginId,
    ListenerUnavailable,
    RecoveryRequired,
    SecretMissing,
    SecretStoreUnavailable,
    SnapshotIo,
    UnsupportedSnapshotVersion,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthFlowError {
    pub code: OAuthFlowErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_id: Option<String>,
}

impl OAuthFlowError {
    fn new(code: OAuthFlowErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
            login_id: None,
        }
    }

    fn for_login(mut self, login_id: &str) -> Self {
        self.login_id = Some(login_id.to_string());
        self
    }
}

impl fmt::Display for OAuthFlowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for OAuthFlowError {}

pub struct OAuthFlowManager<B, E> {
    inner: Arc<OAuthFlowInner<B, E>>,
}

impl<B, E> Clone for OAuthFlowManager<B, E> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<B, E> fmt::Debug for OAuthFlowManager<B, E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OAuthFlowManager")
            .field("root", &self.inner.root)
            .field("listener_count", &lock(&self.inner.listeners).len())
            .finish()
    }
}

impl<B, E> OAuthFlowManager<B, E>
where
    B: SecretBackend + Send + Sync + 'static,
    E: OAuthFlowEventSink,
{
    pub fn new(root: PathBuf, secrets: B, events: E) -> Self {
        Self {
            inner: Arc::new(OAuthFlowInner {
                root,
                secrets,
                events,
                listeners: Mutex::new(HashMap::new()),
                mutation: Mutex::new(()),
            }),
        }
    }

    pub async fn start(&self, oauth: &CodexOAuthClient) -> Result<OAuthFlowStart, OAuthFlowError> {
        let now_ms = now_ms();
        for snapshot in load_snapshots(&self.inner.root)? {
            if snapshot.pending.expires_at_ms() <= now_ms {
                self.inner.cleanup(&snapshot.login_id)?;
                continue;
            }
            let port = callback_port(&snapshot.pending)?;
            if !CODEX_OAUTH_CALLBACK_PORTS.contains(&port) {
                self.inner.cleanup(&snapshot.login_id)?;
                continue;
            }
            if snapshot.status == OAuthFlowStatus::CallbackReceived {
                return Ok(snapshot.start());
            }
            if lock(&self.inner.listeners).contains_key(&snapshot.login_id) {
                return Ok(snapshot.start());
            }
            if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)).await {
                self.spawn_listener(listener, snapshot.clone(), now_ms);
                self.inner
                    .emit(&snapshot.login_id, OAuthFlowStatus::Pending);
                return Ok(snapshot.start());
            }
        }

        let listener = bind_callback_listener().await?;
        let port = listener
            .local_addr()
            .map_err(|_| {
                OAuthFlowError::new(
                    OAuthFlowErrorCode::ListenerUnavailable,
                    "OAuth callback listener address is unavailable",
                )
            })?
            .port();
        let login_id = Uuid::new_v4().hyphenated().to_string();
        let start = oauth.begin(port, now_ms).map_err(|_| {
            OAuthFlowError::new(
                OAuthFlowErrorCode::ListenerUnavailable,
                "OAuth login could not be initialized",
            )
        })?;
        let snapshot = PendingSnapshot {
            version: SNAPSHOT_VERSION,
            login_id: login_id.clone(),
            authorization_url: start.authorization_url().to_string(),
            callback_secret_ref: callback_secret_ref(&login_id),
            status: OAuthFlowStatus::Pending,
            pending: start.into_pending(),
        };
        write_snapshot(&self.inner.root, &snapshot)?;
        self.spawn_listener(listener, snapshot.clone(), now_ms);
        self.inner.emit(&login_id, OAuthFlowStatus::Pending);
        Ok(snapshot.start())
    }

    pub async fn resume(&self, login_id: &str) -> Result<OAuthFlowStart, OAuthFlowError> {
        let login_id = validate_login_id(login_id)?;
        let snapshot = read_snapshot(&self.inner.root, &login_id)?;
        if snapshot.pending.expires_at_ms() <= now_ms() {
            self.inner.cleanup(&login_id)?;
            return Err(
                OAuthFlowError::new(OAuthFlowErrorCode::Expired, "OAuth login expired")
                    .for_login(&login_id),
            );
        }
        let port = callback_port(&snapshot.pending)?;
        if !CODEX_OAUTH_CALLBACK_PORTS.contains(&port) {
            self.inner.cleanup(&login_id)?;
            return Err(OAuthFlowError::new(
                OAuthFlowErrorCode::Expired,
                "OAuth login must be restarted",
            )
            .for_login(&login_id));
        }
        if snapshot.status == OAuthFlowStatus::Pending
            && !lock(&self.inner.listeners).contains_key(&login_id)
        {
            let listener = TcpListener::bind(("127.0.0.1", port)).await.map_err(|_| {
                OAuthFlowError::new(
                    OAuthFlowErrorCode::CallbackPortUnavailable,
                    "OAuth callback port is unavailable",
                )
                .for_login(&login_id)
            })?;
            self.spawn_listener(listener, snapshot.clone(), now_ms());
        }
        self.inner.emit(&login_id, snapshot.status);
        Ok(snapshot.start())
    }

    pub fn status(&self, login_id: &str) -> Result<OAuthFlowStart, OAuthFlowError> {
        let login_id = validate_login_id(login_id)?;
        read_snapshot(&self.inner.root, &login_id).map(|snapshot| snapshot.start())
    }

    pub async fn submit_manual_callback(
        &self,
        login_id: &str,
        callback_url: &str,
    ) -> Result<(), OAuthFlowError> {
        let login_id = validate_login_id(login_id)?;
        self.inner.accept_callback(&login_id, callback_url)?;
        self.stop_listener(&login_id).await;
        Ok(())
    }

    pub fn exchange_material(
        &self,
        login_id: &str,
    ) -> Result<OAuthExchangeMaterial, OAuthFlowError> {
        let login_id = validate_login_id(login_id)?;
        let _mutation = lock(&self.inner.mutation);
        let snapshot = read_snapshot(&self.inner.root, &login_id)?;
        if snapshot.status != OAuthFlowStatus::CallbackReceived {
            return Err(OAuthFlowError::new(
                OAuthFlowErrorCode::SecretMissing,
                "OAuth callback has not been received",
            )
            .for_login(&login_id));
        }
        let callback_url = self
            .inner
            .secrets
            .load(&snapshot.callback_secret_ref)
            .map_err(|_| {
                OAuthFlowError::new(
                    OAuthFlowErrorCode::SecretStoreUnavailable,
                    "OAuth callback secret store is unavailable",
                )
                .for_login(&login_id)
            })?
            .ok_or_else(|| {
                OAuthFlowError::new(
                    OAuthFlowErrorCode::SecretMissing,
                    "OAuth callback secret is missing",
                )
                .for_login(&login_id)
            })?;
        let callback = snapshot
            .pending
            .parse_callback(&callback_url, now_ms())
            .map_err(|_| {
                OAuthFlowError::new(
                    OAuthFlowErrorCode::CallbackInvalid,
                    "OAuth callback is invalid",
                )
                .for_login(&login_id)
            })?;
        Ok(OAuthExchangeMaterial {
            pending: snapshot.pending,
            callback,
        })
    }

    pub async fn cancel(&self, login_id: &str) -> Result<(), OAuthFlowError> {
        let login_id = validate_login_id(login_id)?;
        self.stop_listener(&login_id).await;
        self.inner.cleanup(&login_id)?;
        self.inner.emit(&login_id, OAuthFlowStatus::Canceled);
        Ok(())
    }

    pub async fn complete(&self, login_id: &str) -> Result<(), OAuthFlowError> {
        let login_id = validate_login_id(login_id)?;
        self.stop_listener(&login_id).await;
        self.inner.cleanup(&login_id)?;
        self.inner.emit(&login_id, OAuthFlowStatus::Completed);
        Ok(())
    }

    #[cfg(test)]
    pub async fn shutdown(&self) {
        let controls = {
            let mut listeners = lock(&self.inner.listeners);
            listeners
                .drain()
                .map(|(_, control)| control)
                .collect::<Vec<_>>()
        };
        for mut control in controls {
            if let Some(shutdown) = control.shutdown.take() {
                let _ = shutdown.send(());
            }
            let _ = control.task.await;
        }
    }

    fn spawn_listener(&self, listener: TcpListener, snapshot: PendingSnapshot, started_at_ms: u64) {
        let login_id = snapshot.login_id.clone();
        let inner = Arc::clone(&self.inner);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (start_tx, start_rx) = oneshot::channel();
        let task_login_id = login_id.clone();
        let task = tokio::spawn(async move {
            if start_rx.await.is_ok() {
                run_listener(
                    Arc::clone(&inner),
                    listener,
                    snapshot,
                    started_at_ms,
                    shutdown_rx,
                )
                .await;
            }
            lock(&inner.listeners).remove(&task_login_id);
        });
        lock(&self.inner.listeners).insert(
            login_id,
            ListenerControl {
                shutdown: Some(shutdown_tx),
                task,
            },
        );
        let _ = start_tx.send(());
    }

    async fn stop_listener(&self, login_id: &str) {
        let control = lock(&self.inner.listeners).remove(login_id);
        if let Some(mut control) = control {
            if let Some(shutdown) = control.shutdown.take() {
                let _ = shutdown.send(());
            }
            let _ = control.task.await;
        }
    }
}

struct OAuthFlowInner<B, E> {
    root: PathBuf,
    secrets: B,
    events: E,
    listeners: Mutex<HashMap<String, ListenerControl>>,
    mutation: Mutex<()>,
}

impl<B, E> OAuthFlowInner<B, E>
where
    B: SecretBackend,
    E: OAuthFlowEventSink,
{
    fn accept_callback(&self, login_id: &str, callback_url: &str) -> Result<(), OAuthFlowError> {
        let _mutation = lock(&self.mutation);
        let mut snapshot = read_snapshot(&self.root, login_id)?;
        if snapshot.status == OAuthFlowStatus::CallbackReceived {
            return Err(OAuthFlowError::new(
                OAuthFlowErrorCode::CallbackAlreadyReceived,
                "OAuth callback was already received",
            )
            .for_login(login_id));
        }
        snapshot
            .pending
            .parse_callback(callback_url, now_ms())
            .map_err(|_| {
                OAuthFlowError::new(
                    OAuthFlowErrorCode::CallbackInvalid,
                    "OAuth callback is invalid",
                )
                .for_login(login_id)
            })?;
        self.secrets
            .save(&snapshot.callback_secret_ref, callback_url)
            .map_err(|_| {
                OAuthFlowError::new(
                    OAuthFlowErrorCode::SecretStoreUnavailable,
                    "OAuth callback secret could not be saved",
                )
                .for_login(login_id)
            })?;
        snapshot.status = OAuthFlowStatus::CallbackReceived;
        if let Err(error) = write_snapshot(&self.root, &snapshot) {
            if self.secrets.delete(&snapshot.callback_secret_ref).is_err() {
                return Err(OAuthFlowError::new(
                    OAuthFlowErrorCode::RecoveryRequired,
                    "OAuth callback cleanup requires recovery",
                )
                .for_login(login_id));
            }
            return Err(error.for_login(login_id));
        }
        self.emit(login_id, OAuthFlowStatus::CallbackReceived);
        Ok(())
    }

    fn cleanup(&self, login_id: &str) -> Result<(), OAuthFlowError> {
        let _mutation = lock(&self.mutation);
        let secret_ref = callback_secret_ref(login_id);
        self.secrets.delete(&secret_ref).map_err(|_| {
            OAuthFlowError::new(
                OAuthFlowErrorCode::SecretStoreUnavailable,
                "OAuth callback secret could not be cleared",
            )
            .for_login(login_id)
        })?;
        remove_snapshot(&snapshot_path(&self.root, login_id)?).map_err(|_| {
            OAuthFlowError::new(
                OAuthFlowErrorCode::CleanupIncomplete,
                "OAuth pending snapshot cleanup is incomplete",
            )
            .for_login(login_id)
        })?;
        Ok(())
    }

    fn emit(&self, login_id: &str, status: OAuthFlowStatus) {
        self.events.emit(OAuthFlowEvent {
            login_id: login_id.to_string(),
            status,
        });
    }
}

struct ListenerControl {
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingSnapshot {
    version: u32,
    login_id: String,
    authorization_url: String,
    callback_secret_ref: String,
    status: OAuthFlowStatus,
    pending: OAuthPendingSession,
}

impl PendingSnapshot {
    fn start(&self) -> OAuthFlowStart {
        OAuthFlowStart {
            login_id: self.login_id.clone(),
            authorization_url: self.authorization_url.clone(),
            redirect_uri: self.pending.redirect_uri().to_string(),
            expires_at_ms: self.pending.expires_at_ms(),
            status: self.status,
        }
    }
}

async fn run_listener<B, E>(
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
    let target = match read_request_target(&mut stream).await {
        Ok(target) => target,
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
    let Ok(callback_url) = callback_url(&snapshot.pending, &target) else {
        let _ = write_response(&mut stream, 400, "Invalid OAuth callback request.").await;
        return RequestOutcome::Rejected;
    };
    match inner.accept_callback(&snapshot.login_id, &callback_url) {
        Ok(()) => {
            let _ = write_callback_success(&mut stream).await;
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

async fn read_request_target(stream: &mut TcpStream) -> Result<String, RequestReadError> {
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
        Ok(target.to_string())
    };
    tokio::time::timeout(REQUEST_READ_TIMEOUT, read)
        .await
        .map_err(|_| RequestReadError::Io)?
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

async fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> io::Result<()> {
    write_http_response(stream, status, "text/plain; charset=utf-8", body).await
}

async fn write_callback_success(stream: &mut TcpStream) -> io::Result<()> {
    write_http_response(
        stream,
        200,
        "text/html; charset=utf-8",
        CALLBACK_SUCCESS_HTML,
    )
    .await
}

async fn write_http_response(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &str,
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        413 => "Content Too Large",
        _ => "Internal Server Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

fn load_snapshots(root: &Path) -> Result<Vec<PendingSnapshot>, OAuthFlowError> {
    let directory = pending_directory(root);
    ensure_pending_directory(&directory)?;
    let mut snapshots = Vec::new();
    for entry in fs::read_dir(&directory).map_err(|_| snapshot_io())? {
        let entry = entry.map_err(|_| snapshot_io())?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("tmp") {
            remove_snapshot(&path).map_err(|_| recovery_required())?;
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            return Err(recovery_required());
        }
        let login_id = path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(recovery_required)?;
        let login_id = validate_login_id(login_id).map_err(|_| recovery_required())?;
        snapshots.push(read_snapshot(root, &login_id)?);
    }
    snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.pending.created_at_ms()));
    Ok(snapshots)
}

fn read_snapshot(root: &Path, login_id: &str) -> Result<PendingSnapshot, OAuthFlowError> {
    let login_id = validate_login_id(login_id)?;
    let path = snapshot_path(root, &login_id)?;
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            OAuthFlowError::new(
                OAuthFlowErrorCode::RecoveryRequired,
                "OAuth pending snapshot was not found",
            )
            .for_login(&login_id)
        } else {
            snapshot_io().for_login(&login_id)
        }
    })?;
    if !metadata.file_type().is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_SNAPSHOT_BYTES
    {
        return Err(recovery_required().for_login(&login_id));
    }
    let bytes = fs::read(&path).map_err(|_| snapshot_io().for_login(&login_id))?;
    let snapshot: PendingSnapshot =
        serde_json::from_slice(&bytes).map_err(|_| recovery_required().for_login(&login_id))?;
    validate_snapshot(&snapshot, &login_id)?;
    Ok(snapshot)
}

fn validate_snapshot(
    snapshot: &PendingSnapshot,
    expected_login_id: &str,
) -> Result<(), OAuthFlowError> {
    if snapshot.version != SNAPSHOT_VERSION {
        return Err(OAuthFlowError::new(
            OAuthFlowErrorCode::UnsupportedSnapshotVersion,
            "OAuth pending snapshot version is unsupported",
        )
        .for_login(expected_login_id));
    }
    if snapshot.login_id != expected_login_id
        || snapshot.callback_secret_ref != callback_secret_ref(expected_login_id)
        || !matches!(
            snapshot.status,
            OAuthFlowStatus::Pending | OAuthFlowStatus::CallbackReceived
        )
        || snapshot.pending.created_at_ms() == 0
        || callback_port(&snapshot.pending).is_err()
    {
        return Err(recovery_required().for_login(expected_login_id));
    }
    let authorization_url = Url::parse(&snapshot.authorization_url)
        .map_err(|_| recovery_required().for_login(expected_login_id))?;
    let authorization_endpoint = Url::parse(AUTHORIZATION_ENDPOINT)
        .map_err(|_| recovery_required().for_login(expected_login_id))?;
    if authorization_url.scheme() != authorization_endpoint.scheme()
        || authorization_url.host_str() != authorization_endpoint.host_str()
        || authorization_url.port().is_some()
        || authorization_url.path() != authorization_endpoint.path()
        || !authorization_url.username().is_empty()
        || authorization_url.password().is_some()
        || authorization_url.fragment().is_some()
    {
        return Err(recovery_required().for_login(expected_login_id));
    }
    let mut redirect_uri_count = 0;
    for (key, value) in authorization_url.query_pairs() {
        if [
            "code",
            "access_token",
            "refresh_token",
            "id_token",
            "token",
            "client_secret",
            "authorization",
            "password",
            "api_key",
        ]
        .iter()
        .any(|sensitive| key.eq_ignore_ascii_case(sensitive))
        {
            return Err(recovery_required().for_login(expected_login_id));
        }
        if key == "redirect_uri" {
            redirect_uri_count += 1;
            if value != snapshot.pending.redirect_uri() {
                return Err(recovery_required().for_login(expected_login_id));
            }
        }
    }
    if redirect_uri_count != 1 {
        return Err(recovery_required().for_login(expected_login_id));
    }
    Ok(())
}

fn write_snapshot(root: &Path, snapshot: &PendingSnapshot) -> Result<(), OAuthFlowError> {
    validate_snapshot(snapshot, &snapshot.login_id)?;
    let path = snapshot_path(root, &snapshot.login_id)?;
    ensure_pending_directory(path.parent().ok_or_else(recovery_required)?)?;
    ensure_regular_or_missing(&path)?;
    ensure_regular_or_missing(&path.with_extension("tmp"))?;
    let mut content = serde_json::to_string_pretty(snapshot).map_err(|_| recovery_required())?;
    content.push('\n');
    if content.len() as u64 > MAX_SNAPSHOT_BYTES {
        return Err(recovery_required().for_login(&snapshot.login_id));
    }
    crate::files::atomic_write(&path, &content)
        .map_err(|_| snapshot_io().for_login(&snapshot.login_id))
}

fn ensure_pending_directory(path: &Path) -> Result<(), OAuthFlowError> {
    fs::create_dir_all(path).map_err(|_| snapshot_io())?;
    let metadata = fs::symlink_metadata(path).map_err(|_| snapshot_io())?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        Err(recovery_required())
    } else {
        Ok(())
    }
}

fn ensure_regular_or_missing(path: &Path) -> Result<(), OAuthFlowError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            Ok(())
        }
        Ok(_) => Err(recovery_required()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(snapshot_io()),
    }
}

fn remove_snapshot(path: &Path) -> Result<(), ()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(path).map_err(|_| ())
        }
        Ok(_) => Err(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(()),
    }
}

fn snapshot_path(root: &Path, login_id: &str) -> Result<PathBuf, OAuthFlowError> {
    let login_id = validate_login_id(login_id)?;
    Ok(pending_directory(root).join(format!("{login_id}.json")))
}

fn pending_directory(root: &Path) -> PathBuf {
    root.join("oauth_pending")
}

fn callback_secret_ref(login_id: &str) -> String {
    format!("oauth-callback:{login_id}")
}

fn validate_login_id(login_id: &str) -> Result<String, OAuthFlowError> {
    let login_id = login_id.trim();
    let uuid = Uuid::parse_str(login_id).map_err(|_| {
        OAuthFlowError::new(
            OAuthFlowErrorCode::InvalidLoginId,
            "OAuth login id is invalid",
        )
    })?;
    let canonical = uuid.hyphenated().to_string();
    if login_id != canonical || !login_id.is_ascii() {
        Err(OAuthFlowError::new(
            OAuthFlowErrorCode::InvalidLoginId,
            "OAuth login id is invalid",
        ))
    } else {
        Ok(canonical)
    }
}

fn callback_port(pending: &OAuthPendingSession) -> Result<u16, OAuthFlowError> {
    let redirect = Url::parse(pending.redirect_uri()).map_err(|_| recovery_required())?;
    if redirect.scheme() != "http"
        || redirect.host_str() != Some("localhost")
        || redirect.path() != CALLBACK_PATH
        || redirect.query().is_some()
        || redirect.fragment().is_some()
    {
        return Err(recovery_required());
    }
    redirect.port().ok_or_else(recovery_required)
}

async fn bind_callback_listener() -> Result<TcpListener, OAuthFlowError> {
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

fn snapshot_io() -> OAuthFlowError {
    OAuthFlowError::new(
        OAuthFlowErrorCode::SnapshotIo,
        "OAuth pending snapshot could not be accessed",
    )
}

fn recovery_required() -> OAuthFlowError {
    OAuthFlowError::new(
        OAuthFlowErrorCode::RecoveryRequired,
        "OAuth pending state requires recovery",
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(1)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::super::import_session::SecretBackendError;
    use super::*;
    use std::collections::BTreeMap;

    static TEST_OAUTH_PORT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[derive(Clone, Default)]
    struct MemorySecrets(Arc<Mutex<BTreeMap<String, String>>>);

    impl SecretBackend for MemorySecrets {
        fn save(&self, secret_ref: &str, value: &str) -> Result<(), SecretBackendError> {
            lock(&self.0).insert(secret_ref.to_string(), value.to_string());
            Ok(())
        }

        fn load(&self, secret_ref: &str) -> Result<Option<String>, SecretBackendError> {
            Ok(lock(&self.0).get(secret_ref).cloned())
        }

        fn delete(&self, secret_ref: &str) -> Result<(), SecretBackendError> {
            lock(&self.0).remove(secret_ref);
            Ok(())
        }
    }

    impl MemorySecrets {
        fn contains(&self, secret_ref: &str) -> bool {
            lock(&self.0).contains_key(secret_ref)
        }
    }

    #[derive(Clone, Default)]
    struct Events(Arc<Mutex<Vec<OAuthFlowEvent>>>);

    impl OAuthFlowEventSink for Events {
        fn emit(&self, event: OAuthFlowEvent) {
            lock(&self.0).push(event);
        }
    }

    impl Events {
        fn has(&self, login_id: &str, status: OAuthFlowStatus) -> bool {
            lock(&self.0)
                .iter()
                .any(|event| event.login_id == login_id && event.status == status)
        }
    }

    #[tokio::test]
    async fn loopback_callback_validates_state_and_stores_only_secret_material() {
        let _port_guard = TEST_OAUTH_PORT_LOCK.lock().await;
        let root = test_root("callback-success");
        let secrets = MemorySecrets::default();
        let events = Events::default();
        let manager = OAuthFlowManager::new(root.clone(), secrets.clone(), events.clone());
        let start = manager
            .start(&CodexOAuthClient::new().unwrap())
            .await
            .unwrap();
        assert!(CODEX_OAUTH_CALLBACK_PORTS
            .contains(&Url::parse(&start.redirect_uri).unwrap().port().unwrap()));
        let callback = callback_url_for(&start, "authorization-code", None);

        let response = send_callback(&start.redirect_uri, &request_target(&callback)).await;
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("Content-Type: text/html; charset=utf-8"));
        assert!(response.contains("window.close()"));
        assert!(!response.contains("authorization-code"));
        wait_until(|| events.has(&start.login_id, OAuthFlowStatus::CallbackReceived)).await;
        assert!(secrets.contains(&callback_secret_ref(&start.login_id)));
        let material = manager.exchange_material(&start.login_id).unwrap();
        assert!(!format!("{material:?}").contains("authorization-code"));
        let (pending, callback) = material.into_parts();
        assert_eq!(pending.redirect_uri(), start.redirect_uri);
        assert!(!format!("{pending:?} {callback:?}").contains("authorization-code"));
        let snapshot = fs::read_to_string(snapshot_path(&root, &start.login_id).unwrap()).unwrap();
        assert!(!snapshot.contains("authorization-code"));
        assert!(!snapshot.contains("access-token"));
        assert!(!format!("{manager:?} {start:?}").contains("authorization-code"));
        manager.complete(&start.login_id).await.unwrap();
        remove_root(&root);
    }

    #[tokio::test]
    async fn state_mismatch_is_rejected_and_listener_remains_cancelable() {
        let _port_guard = TEST_OAUTH_PORT_LOCK.lock().await;
        let root = test_root("state-mismatch");
        let secrets = MemorySecrets::default();
        let events = Events::default();
        let manager = OAuthFlowManager::new(root.clone(), secrets.clone(), events.clone());
        let start = manager
            .start(&CodexOAuthClient::new().unwrap())
            .await
            .unwrap();
        let callback = callback_url_for(&start, "authorization-code", Some("wrong-state"));

        let response = send_callback(&start.redirect_uri, &request_target(&callback)).await;
        assert!(response.starts_with("HTTP/1.1 400"));
        wait_until(|| events.has(&start.login_id, OAuthFlowStatus::CallbackRejected)).await;
        assert!(!secrets.contains(&callback_secret_ref(&start.login_id)));
        assert_eq!(
            manager.status(&start.login_id).unwrap().status,
            OAuthFlowStatus::Pending
        );
        manager.cancel(&start.login_id).await.unwrap();
        remove_root(&root);
    }

    #[tokio::test]
    async fn manual_callback_and_restart_resume_use_the_known_pending_session() {
        let _port_guard = TEST_OAUTH_PORT_LOCK.lock().await;
        let root = test_root("manual-restart");
        let secrets = MemorySecrets::default();
        let first_events = Events::default();
        let first = OAuthFlowManager::new(root.clone(), secrets.clone(), first_events);
        let start = first
            .start(&CodexOAuthClient::new().unwrap())
            .await
            .unwrap();
        first.shutdown().await;

        let second_events = Events::default();
        let second = OAuthFlowManager::new(root.clone(), secrets.clone(), second_events.clone());
        let resumed = second.resume(&start.login_id).await.unwrap();
        assert_eq!(resumed.redirect_uri, start.redirect_uri);
        let callback = callback_url_for(&resumed, "manual-code", None);
        second
            .submit_manual_callback(&resumed.login_id, callback.as_str())
            .await
            .unwrap();
        assert_eq!(
            second.status(&resumed.login_id).unwrap().status,
            OAuthFlowStatus::CallbackReceived
        );
        assert!(second_events.has(&resumed.login_id, OAuthFlowStatus::CallbackReceived));
        second.complete(&resumed.login_id).await.unwrap();
        remove_root(&root);
    }

    #[tokio::test]
    async fn cancel_removes_snapshot_and_releases_callback_port() {
        let _port_guard = TEST_OAUTH_PORT_LOCK.lock().await;
        let root = test_root("cancel");
        let manager =
            OAuthFlowManager::new(root.clone(), MemorySecrets::default(), Events::default());
        let start = manager
            .start(&CodexOAuthClient::new().unwrap())
            .await
            .unwrap();
        let port = Url::parse(&start.redirect_uri).unwrap().port().unwrap();
        manager.cancel(&start.login_id).await.unwrap();

        assert!(!snapshot_path(&root, &start.login_id).unwrap().exists());
        let rebound = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
        drop(rebound);
        remove_root(&root);
    }

    #[tokio::test]
    async fn traversal_oversized_requests_and_corrupt_snapshots_fail_safely() {
        let _port_guard = TEST_OAUTH_PORT_LOCK.lock().await;
        let root = test_root("unsafe");
        let manager =
            OAuthFlowManager::new(root.clone(), MemorySecrets::default(), Events::default());
        assert_eq!(
            manager.resume("../oauth.json").await.unwrap_err().code,
            OAuthFlowErrorCode::InvalidLoginId
        );

        let start = manager
            .start(&CodexOAuthClient::new().unwrap())
            .await
            .unwrap();
        let oversized = format!(
            "GET /auth/callback?{} HTTP/1.1\r\nHost: localhost\r\n\r\n",
            "x".repeat(MAX_REQUEST_HEADER_BYTES)
        );
        let response = send_raw(&start.redirect_uri, &oversized).await;
        assert!(response.starts_with("HTTP/1.1 413"));
        assert_eq!(
            manager.status(&start.login_id).unwrap().status,
            OAuthFlowStatus::Pending
        );
        manager.cancel(&start.login_id).await.unwrap();

        let corrupt_id = Uuid::new_v4().hyphenated().to_string();
        ensure_pending_directory(&pending_directory(&root)).unwrap();
        fs::write(
            snapshot_path(&root, &corrupt_id).unwrap(),
            r#"{"version":1,"authorizationCode":"raw-code"}"#,
        )
        .unwrap();
        let error = manager.resume(&corrupt_id).await.unwrap_err();
        assert_eq!(error.code, OAuthFlowErrorCode::RecoveryRequired);
        assert!(!format!("{error:?} {error}").contains("raw-code"));
        remove_root(&root);
    }

    #[test]
    fn persisted_authorization_url_is_strictly_validated() {
        let login_id = Uuid::new_v4().hyphenated().to_string();
        let oauth_start = CodexOAuthClient::new()
            .unwrap()
            .begin(1455, 10_000)
            .unwrap();
        let authorization_url = oauth_start.authorization_url().to_string();
        let pending = oauth_start.into_pending();
        let snapshot = PendingSnapshot {
            version: SNAPSHOT_VERSION,
            login_id: login_id.clone(),
            authorization_url,
            callback_secret_ref: callback_secret_ref(&login_id),
            status: OAuthFlowStatus::Pending,
            pending,
        };
        validate_snapshot(&snapshot, &login_id).unwrap();

        let redirect_uri = snapshot.pending.redirect_uri();
        let mut wrong_host = Url::parse("https://attacker.invalid/oauth/authorize").unwrap();
        wrong_host
            .query_pairs_mut()
            .append_pair("redirect_uri", redirect_uri);
        let mut credentials =
            Url::parse("https://user:pass@auth.openai.com/oauth/authorize").unwrap();
        credentials
            .query_pairs_mut()
            .append_pair("redirect_uri", redirect_uri);
        let mut fragment = Url::parse(AUTHORIZATION_ENDPOINT).unwrap();
        fragment
            .query_pairs_mut()
            .append_pair("redirect_uri", redirect_uri);
        fragment.set_fragment(Some("callback"));
        let mut sensitive = Url::parse(AUTHORIZATION_ENDPOINT).unwrap();
        sensitive
            .query_pairs_mut()
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("access_token", "secret");
        let mut wrong_redirect = Url::parse(AUTHORIZATION_ENDPOINT).unwrap();
        wrong_redirect
            .query_pairs_mut()
            .append_pair("redirect_uri", "http://localhost:9999/auth/callback");
        let mut duplicate_redirect = Url::parse(AUTHORIZATION_ENDPOINT).unwrap();
        duplicate_redirect
            .query_pairs_mut()
            .append_pair("redirect_uri", redirect_uri)
            .append_pair("redirect_uri", redirect_uri);

        for authorization_url in [
            wrong_host,
            credentials,
            fragment,
            sensitive,
            wrong_redirect,
            duplicate_redirect,
        ] {
            let mut tampered = snapshot.clone();
            tampered.authorization_url = authorization_url.to_string();
            assert_eq!(
                validate_snapshot(&tampered, &login_id).unwrap_err().code,
                OAuthFlowErrorCode::RecoveryRequired
            );
        }
    }

    #[tokio::test]
    async fn exchange_material_requires_received_callback_and_redacts_all_secrets() {
        let _port_guard = TEST_OAUTH_PORT_LOCK.lock().await;
        let root = test_root("exchange-material");
        let manager =
            OAuthFlowManager::new(root.clone(), MemorySecrets::default(), Events::default());
        let start = manager
            .start(&CodexOAuthClient::new().unwrap())
            .await
            .unwrap();
        assert_eq!(
            manager.exchange_material(&start.login_id).unwrap_err().code,
            OAuthFlowErrorCode::SecretMissing
        );
        let callback = callback_url_for(&start, "exchange-secret", None);
        manager
            .submit_manual_callback(&start.login_id, callback.as_str())
            .await
            .unwrap();

        let material = manager.exchange_material(&start.login_id).unwrap();
        assert_eq!(
            format!("{material:?}"),
            "OAuthExchangeMaterial { pending: \"[redacted]\", callback: \"[redacted]\" }"
        );
        let (pending, callback) = material.into_parts();
        assert_eq!(pending.redirect_uri(), start.redirect_uri);
        assert!(!format!("{pending:?} {callback:?}").contains("exchange-secret"));

        manager.complete(&start.login_id).await.unwrap();
        remove_root(&root);
    }

    fn callback_url_for(start: &OAuthFlowStart, code: &str, state_override: Option<&str>) -> Url {
        let authorization = Url::parse(&start.authorization_url).unwrap();
        let state = state_override
            .map(str::to_string)
            .or_else(|| {
                authorization
                    .query_pairs()
                    .find_map(|(key, value)| (key == "state").then(|| value.into_owned()))
            })
            .unwrap();
        let mut callback = Url::parse(&start.redirect_uri).unwrap();
        callback
            .query_pairs_mut()
            .append_pair("code", code)
            .append_pair("state", &state);
        callback
    }

    fn request_target(url: &Url) -> String {
        url.query().map_or_else(
            || url.path().to_string(),
            |query| format!("{}?{query}", url.path()),
        )
    }

    async fn send_callback(redirect_uri: &str, path_and_query: &str) -> String {
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            path_and_query
        );
        send_raw(redirect_uri, &request).await
    }

    async fn send_raw(redirect_uri: &str, request: &str) -> String {
        let port = Url::parse(redirect_uri).unwrap().port().unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        String::from_utf8(response).unwrap()
    }

    async fn wait_until(condition: impl Fn() -> bool) {
        for _ in 0..100 {
            if condition() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("condition was not reached");
    }

    fn test_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-oauth-flow-{label}-{}",
            Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn remove_root(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }
}

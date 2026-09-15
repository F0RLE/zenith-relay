//! Observes the official Codex renderer through its loopback CDP endpoint.
//!
//! This is intentionally a small, evidence-based observer. It records only a
//! stable login-page signal for the account bound to the active Relay profile;
//! it never captures network bodies, page text, cookies, or credentials.

use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::OnceLock;
use std::time::Duration;
use sysinfo::{ProcessesToUpdate, System};
use tauri::{AppHandle, Emitter, Manager};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
};
use url::Url;

use super::{commands::current_time_ms, profiles::codex, state::DesktopState};
use crate::{launcher::is_codex_process, platform::default_codex_home};

const POLL_INTERVAL: Duration = Duration::from_secs(5);
const CDP_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_CDP_RESPONSE_BYTES: usize = 64 * 1024;
static STARTED: OnceLock<()> = OnceLock::new();

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CdpTarget {
    #[serde(rename = "type")]
    target_type: String,
    url: String,
    web_socket_debugger_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AuthPageSnapshot {
    route: String,
    login_ui_signal: bool,
    login_ui_markers: Vec<String>,
}

impl AuthPageSnapshot {
    fn login_signal(&self) -> bool {
        self.route.trim_end_matches('/') == "/login"
            || (self.login_ui_signal
                && self
                    .login_ui_markers
                    .iter()
                    .any(|marker| marker == "login_title")
                && self
                    .login_ui_markers
                    .iter()
                    .any(|marker| marker == "login_primary_action"))
    }
}

const AUTH_SNAPSHOT_SCRIPT: &str = r#"
(() => {
  const normalize = (value) => String(value || "")
    .replace(/^#/, "").split(/[?#]/, 1)[0].replace(/\/+$/, "") || "/";
  const path = normalize(location.pathname);
  const hash = normalize(location.hash);
  const route = hash !== "/" ? hash : path;
  const textOf = (selector) => Array.from(document.querySelectorAll(selector))
    .map((node) => String(node.textContent || "").replace(/\s+/g, " ").trim().slice(0, 160))
    .filter(Boolean);
  const headings = textOf("h1,h2").join(" ");
  const controls = textOf("button,a").join(" ");
  const markers = [];
  if (/sign in to chatgpt|войти в chatgpt|войдите в chatgpt/i.test(headings)) markers.push("login_title");
  if (/continue to sign in|sign in with chatgpt|продолжить вход|войти с помощью chatgpt/i.test(controls)) {
    markers.push("login_primary_action");
  }
  return {
    route: route.slice(0, 160),
    loginUiSignal: markers.length > 0,
    loginUiMarkers: markers,
  };
})()
"#;

pub(crate) fn start(app: AppHandle) {
    if STARTED.set(()).is_err() {
        return;
    }
    tauri::async_runtime::spawn(async move { run(app).await });
}

async fn run(app: AppHandle) {
    let client = match Client::builder().timeout(CDP_TIMEOUT).build() {
        Ok(client) => client,
        Err(_) => return,
    };
    let mut observed_account: Option<String> = None;
    let mut login_streak = 0u8;
    let mut available_streak = 0u8;
    let mut persisted_status: Option<&'static str> = None;
    loop {
        let state = app.state::<DesktopState>();
        let account_id =
            codex::active_managed_account_id(&default_codex_home(), &state.profile_backup_root())
                .ok()
                .flatten();
        if account_id != observed_account {
            observed_account = account_id.clone();
            login_streak = 0;
            available_streak = 0;
            persisted_status = None;
        }
        let Some(account_id) = account_id else {
            tokio::time::sleep(POLL_INTERVAL).await;
            continue;
        };
        let Some(port) = remote_debugging_port() else {
            tokio::time::sleep(POLL_INTERVAL).await;
            continue;
        };
        let targets = query_targets(&client, port).await;
        let mut login_signal = false;
        let mut available_signal = false;
        for target in targets.iter().filter(|target| {
            matches!(target.target_type.as_str(), "page" | "webview")
                && target.url.starts_with("app://-/")
        }) {
            if let Some(snapshot) = query_snapshot(target).await {
                available_signal = true;
                login_signal |= snapshot.login_signal();
            }
        }
        if login_signal {
            login_streak = login_streak.saturating_add(1);
            available_streak = 0;
        } else if available_signal {
            available_streak = available_streak.saturating_add(1);
            login_streak = 0;
        } else {
            login_streak = 0;
            available_streak = 0;
        }
        let stable_status = if login_streak >= 2 {
            Some("login_required")
        } else if available_streak >= 2 {
            Some("available")
        } else {
            None
        };
        if let Some(status) = stable_status {
            if persisted_status != Some(status) {
                // Profile switches can race a slow CDP snapshot. Re-read the
                // active binding before persisting so a login page from the
                // newly selected profile cannot be attributed to the account
                // that was active at the beginning of this poll.
                let current_account_id = codex::active_managed_account_id(
                    &default_codex_home(),
                    &state.profile_backup_root(),
                )
                .ok()
                .flatten();
                if current_account_id.as_deref() != Some(account_id.as_str()) {
                    observed_account = current_account_id;
                    login_streak = 0;
                    available_streak = 0;
                    persisted_status = None;
                    tokio::time::sleep(POLL_INTERVAL).await;
                    continue;
                }
                let redirect_at_ms = if status == "login_required" {
                    Some(current_time_ms())
                } else {
                    state.store().ok().and_then(|store| {
                        store
                            .account(&account_id)
                            .and_then(|a| a.last_client_login_redirect_at_ms)
                    })
                };
                let observation = state.store().and_then(|mut store| {
                    store.update_client_auth_observation(
                        &account_id,
                        Some(status.to_string()),
                        redirect_at_ms,
                    )
                });
                if let Ok(changed) = observation {
                    if changed {
                        let _ = app.emit(
                            "zenith-state-changed",
                            json!({"reason": "client-auth-observation", "accountId": account_id, "status": status}),
                        );
                    }
                    // `Ok(false)` means the desired observation was already
                    // durable (or its account was deleted); only an actual
                    // store error must remain retryable on the next poll.
                    persisted_status = Some(status);
                }
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn remote_debugging_port() -> Option<u16> {
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::All, true);
    unique_remote_debugging_port(system.processes().values().filter_map(|process| {
        // Reuse the launcher identity guard so the observer does not
        // attach to a similarly named CLI, a renderer/helper process, or
        // Relay itself. In particular, current packaged builds may be
        // named `OpenAI.Codex.exe`, which the old name-only check missed.
        if !is_codex_process(process) {
            return None;
        }
        let args = process
            .cmd()
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        if args.iter().any(|arg| arg.starts_with("--type=")) {
            return None;
        }
        parse_debug_port(&args)
    }))
}

/// A CDP login page has no account identifier. When more than one official
/// desktop process exposes a different port, guessing would put a login warning
/// on the wrong Relay account. Observe only an unambiguous process instead.
fn unique_remote_debugging_port(ports: impl IntoIterator<Item = u16>) -> Option<u16> {
    let ports = ports.into_iter().collect::<BTreeSet<_>>();
    (ports.len() == 1)
        .then(|| ports.into_iter().next())
        .flatten()
}

fn parse_debug_port(args: &[String]) -> Option<u16> {
    args.iter().enumerate().find_map(|(index, arg)| {
        let value = arg.strip_prefix("--remote-debugging-port=").or_else(|| {
            if arg == "--remote-debugging-port" {
                args.get(index + 1).map(String::as_str)
            } else {
                None
            }
        });
        value
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|port| *port != 0)
    })
}

async fn query_targets(client: &Client, port: u16) -> Vec<CdpTarget> {
    let url = format!("http://127.0.0.1:{port}/json/list");
    let Ok(response) = client.get(url).send().await else {
        return Vec::new();
    };
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|size| size > MAX_CDP_RESPONSE_BYTES as u64)
    {
        return Vec::new();
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            return Vec::new();
        };
        if chunk.len() > MAX_CDP_RESPONSE_BYTES.saturating_sub(body.len()) {
            return Vec::new();
        }
        body.extend_from_slice(&chunk);
    }
    parse_cdp_targets_body(&body)
}

async fn query_snapshot(target: &CdpTarget) -> Option<AuthPageSnapshot> {
    let websocket_url = target.web_socket_debugger_url.as_deref()?;
    if !is_loopback_websocket_url(websocket_url) {
        return None;
    }
    let websocket_config = WebSocketConfig::default()
        .max_message_size(Some(MAX_CDP_RESPONSE_BYTES))
        .max_frame_size(Some(MAX_CDP_RESPONSE_BYTES));
    let Ok(Ok((mut socket, _))) = timeout(
        CDP_TIMEOUT,
        connect_async_with_config(websocket_url, Some(websocket_config), false),
    )
    .await
    else {
        return None;
    };
    socket
        .send(Message::Text(
            json!({
                "id": 1,
                "method": "Runtime.evaluate",
                "params": {"expression": AUTH_SNAPSHOT_SCRIPT, "returnByValue": true}
            })
            .to_string()
            .into(),
        ))
        .await
        .ok()?;
    loop {
        let Ok(Some(Ok(Message::Text(text)))) = timeout(CDP_TIMEOUT, socket.next()).await else {
            return None;
        };
        let value: serde_json::Value = serde_json::from_str(text.as_ref()).ok()?;
        if value.get("id").and_then(serde_json::Value::as_i64) != Some(1) {
            continue;
        }
        return value
            .pointer("/result/result/value")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok());
    }
}

fn is_loopback_websocket_url(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    let host = url
        .host_str()
        .map(|host| host.trim_start_matches('[').trim_end_matches(']'));
    matches!(url.scheme(), "ws" | "wss")
        && host
            .and_then(|host| host.parse::<std::net::IpAddr>().ok())
            .is_some_and(|host| host.is_loopback())
}

fn parse_cdp_targets_body(body: &[u8]) -> Vec<CdpTarget> {
    if body.len() > MAX_CDP_RESPONSE_BYTES {
        return Vec::new();
    }
    serde_json::from_slice(body).unwrap_or_default()
}

#[cfg(test)]
fn observation_write_completed<E>(result: &std::result::Result<bool, E>) -> bool {
    result.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_signal_requires_route_or_both_login_markers() {
        let route = AuthPageSnapshot {
            route: "/login".into(),
            login_ui_signal: false,
            login_ui_markers: Vec::new(),
        };
        assert!(route.login_signal());
        let ui = AuthPageSnapshot {
            route: "/index.html".into(),
            login_ui_signal: true,
            login_ui_markers: vec!["login_title".into(), "login_primary_action".into()],
        };
        assert!(ui.login_signal());
        let weak = AuthPageSnapshot {
            route: "/index.html".into(),
            login_ui_signal: true,
            login_ui_markers: vec!["login_title".into()],
        };
        assert!(!weak.login_signal());
    }

    #[test]
    fn watchdog_snapshot_includes_russian_login_markers() {
        assert!(AUTH_SNAPSHOT_SCRIPT.contains("войти в chatgpt"));
        assert!(AUTH_SNAPSHOT_SCRIPT.contains("продолжить вход"));
        assert!(!AUTH_SNAPSHOT_SCRIPT.contains("登录"));
    }

    #[test]
    fn debug_port_parser_accepts_both_chromium_forms() {
        assert_eq!(
            parse_debug_port(&["--remote-debugging-port=56140".into()]),
            Some(56140)
        );
        assert_eq!(
            parse_debug_port(&["--remote-debugging-port".into(), "56140".into()]),
            Some(56140)
        );
        assert_eq!(
            parse_debug_port(&["--remote-debugging-port=0".into()]),
            None
        );
    }

    #[test]
    fn watchdog_requires_an_unambiguous_desktop_cdp_port() {
        assert_eq!(unique_remote_debugging_port([]), None);
        assert_eq!(unique_remote_debugging_port([56140]), Some(56140));
        // Electron can expose the same parent port through duplicate process
        // entries, which remains unambiguous.
        assert_eq!(unique_remote_debugging_port([56140, 56140]), Some(56140));
        assert_eq!(unique_remote_debugging_port([56140, 56141]), None);
    }

    #[test]
    fn watchdog_only_connects_to_loopback_cdp_targets() {
        assert!(is_loopback_websocket_url(
            "ws://127.0.0.1:56140/devtools/page/1"
        ));
        assert!(is_loopback_websocket_url(
            "ws://[::1]:56140/devtools/page/1"
        ));
        assert!(!is_loopback_websocket_url(
            "wss://example.test/devtools/page/1"
        ));
        assert!(!is_loopback_websocket_url(
            "http://127.0.0.1:56140/devtools/page/1"
        ));
    }

    #[test]
    fn cdp_target_body_is_bounded_before_deserialization() {
        assert!(parse_cdp_targets_body(&vec![b' '; MAX_CDP_RESPONSE_BYTES + 1]).is_empty());
        assert_eq!(
            parse_cdp_targets_body(
                br#"[{"type":"page","url":"app://-/index.html","webSocketDebuggerUrl":"ws://127.0.0.1:56140/devtools/page/1"}]"#
            )
            .len(),
            1
        );
    }

    #[test]
    fn watchdog_retries_failed_observation_persistence() {
        assert!(observation_write_completed(&Ok::<bool, ()>(false)));
        assert!(!observation_write_completed(&Err::<bool, ()>(())));
    }
}

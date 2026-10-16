use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeSet;
use std::time::Duration;
use sysinfo::{ProcessesToUpdate, System};
use tokio::time::timeout;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
};
use url::Url;

use crate::launcher::is_codex_process;

pub(super) const CDP_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_CDP_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CdpTarget {
    #[serde(rename = "type")]
    target_type: String,
    url: String,
    web_socket_debugger_url: Option<String>,
}

impl CdpTarget {
    pub(super) fn is_codex_app_page(&self) -> bool {
        matches!(self.target_type.as_str(), "page" | "webview") && self.url.starts_with("app://-/")
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct AuthPageSnapshot {
    route: String,
    login_ui_signal: bool,
    login_ui_markers: Vec<String>,
}

impl AuthPageSnapshot {
    pub(super) fn login_signal(&self) -> bool {
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

pub(super) fn remote_debugging_port() -> Option<u16> {
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
        let port_text = arg.strip_prefix("--remote-debugging-port=").or_else(|| {
            if arg == "--remote-debugging-port" {
                args.get(index + 1).map(String::as_str)
            } else {
                None
            }
        });
        port_text
            .and_then(|port_text| port_text.parse::<u16>().ok())
            .filter(|port| *port != 0)
    })
}

pub(super) async fn query_targets(client: &Client, port: u16) -> Vec<CdpTarget> {
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
    let mut response_bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            return Vec::new();
        };
        if chunk.len() > MAX_CDP_RESPONSE_BYTES.saturating_sub(response_bytes.len()) {
            return Vec::new();
        }
        response_bytes.extend_from_slice(&chunk);
    }
    parse_cdp_targets_body(&response_bytes)
}

pub(super) async fn query_snapshot(target: &CdpTarget) -> Option<AuthPageSnapshot> {
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
        let response_document: serde_json::Value = serde_json::from_str(text.as_ref()).ok()?;
        if response_document
            .get("id")
            .and_then(serde_json::Value::as_i64)
            != Some(1)
        {
            continue;
        }
        return response_document
            .pointer("/result/result/value")
            .cloned()
            .and_then(|snapshot_value| serde_json::from_value(snapshot_value).ok());
    }
}

fn is_loopback_websocket_url(url_text: &str) -> bool {
    let Ok(url) = Url::parse(url_text) else {
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

fn parse_cdp_targets_body(response_body: &[u8]) -> Vec<CdpTarget> {
    if response_body.len() > MAX_CDP_RESPONSE_BYTES {
        return Vec::new();
    }
    serde_json::from_slice(response_body).unwrap_or_default()
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
        // A Chromium desktop wrapper can expose the same parent port through
        // duplicate process entries, which remains unambiguous.
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
}

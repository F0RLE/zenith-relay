use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use super::{SIGN_IN_VIEW, main_window};
use crate::local_pool::error::{ErrorCode, LocalPoolError};

pub(super) const SCRIPT: &str = include_str!("edit_menu.js");

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum EditAction {
    Read,
    Cut,
    Paste,
    SelectAll,
    Dismiss,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EditResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Request {
    kind: String,
    token: String,
    x: f64,
    y: f64,
    can_copy: bool,
    can_cut: bool,
    can_paste: bool,
    can_select_all: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MenuEvent<'a> {
    login_id: &'a str,
    request_id: &'a str,
    x: f64,
    y: f64,
    can_copy: bool,
    can_cut: bool,
    can_paste: bool,
    can_select_all: bool,
}

pub(super) struct Pending {
    id: String,
    opened: Instant,
    request: Request,
}

impl Pending {
    pub(super) fn matches(&self, id: &str) -> bool {
        self.id == id
    }

    pub(super) fn script(
        &self,
        id: &str,
        action: EditAction,
        value: Option<&str>,
    ) -> Result<String, LocalPoolError> {
        let allowed = match action {
            EditAction::Read => self.request.can_copy,
            EditAction::Cut => self.request.can_cut,
            EditAction::Paste => self.request.can_paste,
            EditAction::SelectAll => self.request.can_select_all,
            EditAction::Dismiss => true,
        };
        if self.id != id
            || (action != EditAction::Dismiss && self.opened.elapsed() > Duration::from_secs(120))
            || !allowed
            || value.is_some_and(|text| action != EditAction::Paste || text.len() > 1_048_576)
            || (action == EditAction::Paste && value.is_none())
        {
            return Err(unavailable());
        }
        let input = serde_json::json!({ "token": self.request.token, "action": action, "value": value.unwrap_or("") });
        Ok(format!(
            "window.__relaySignInEdit?.({input}) ?? {{ok:false}}"
        ))
    }
}

pub(super) fn requested(app: &AppHandle, generation: u64, login_id: &str, raw: &str) {
    let Some(request) = parse(raw) else { return };
    let Some(parent) = main_window(app) else {
        return;
    };
    SIGN_IN_VIEW.with_borrow_mut(|slot| {
        let Some(native) = slot
            .as_mut()
            .filter(|view| view.generation == generation && view.visible)
        else {
            return;
        };
        let pending = Pending {
            id: uuid::Uuid::new_v4().to_string(),
            opened: Instant::now(),
            request,
        };
        let event = MenuEvent {
            login_id,
            request_id: &pending.id,
            x: pending.request.x,
            y: pending.request.y,
            can_copy: pending.request.can_copy,
            can_cut: pending.request.can_cut,
            can_paste: pending.request.can_paste,
            can_select_all: pending.request.can_select_all,
        };
        if app
            .emit_to(
                tauri::EventTarget::webview_window(parent.label()),
                "relay-oauth-edit-menu",
                event,
            )
            .is_ok()
        {
            native.menu = Some(pending);
        }
    });
}

fn parse(raw: &str) -> Option<Request> {
    if raw.len() > 1024 {
        return None;
    }
    let request: Request = serde_json::from_str(raw).ok()?;
    (request.kind == "relayEditMenu"
        && uuid::Uuid::parse_str(&request.token).is_ok()
        && [request.x, request.y]
            .iter()
            .all(|value| value.is_finite() && (0.0..=16_384.0).contains(value)))
    .then_some(request)
}

pub(super) fn unavailable() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::InvalidState,
        "The sign-in selection is no longer available.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> String {
        serde_json::json!({"kind":"relayEditMenu","token":uuid::Uuid::new_v4().to_string(),"x":10,"y":20,"canCopy":true,"canCut":false,"canPaste":true,"canSelectAll":true}).to_string()
    }
    #[test]
    fn remote_messages_cannot_invoke_commands_or_supply_text() {
        assert!(parse(&request()).is_some());
        assert!(parse(r#"{"command":"read_clipboard"}"#).is_none());
        let mut extra: serde_json::Value = serde_json::from_str(&request()).unwrap();
        extra["text"] = "synthetic secret".into();
        assert!(parse(&extra.to_string()).is_none());
        assert!(parse(&"x".repeat(1025)).is_none());
    }
    #[test]
    fn actions_bind_to_the_open_menu_and_available_operations() {
        let pending = Pending {
            id: "current".into(),
            opened: Instant::now(),
            request: parse(&request()).unwrap(),
        };
        assert!(pending.script("other", EditAction::Read, None).is_err());
        assert!(pending.script("current", EditAction::Cut, None).is_err());
        assert!(pending.script("current", EditAction::Paste, None).is_err());
        assert!(
            pending
                .script("current", EditAction::Read, Some("value"))
                .is_err()
        );
        let script = pending
            .script("current", EditAction::Paste, Some("'\"\\\n"))
            .unwrap();
        assert!(script.contains("\\n"));
        assert!(pending.script("current", EditAction::Dismiss, None).is_ok());
    }
}

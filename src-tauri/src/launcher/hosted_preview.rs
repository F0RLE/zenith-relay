use tauri::{
    AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, webview::NewWindowResponse,
};
use url::Url;

const PREVIEW_WINDOW: &str = "hosted-preview";
const PREVIEW_URL: &str = "http://127.0.0.1:1441/preview.html";

fn preview_allowed(debug_build: bool, caller: &str) -> bool {
    debug_build && caller == "main"
}

fn navigation_allowed(url: &Url) -> bool {
    url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port() == Some(1441)
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/preview.html"
}

#[tauri::command]
pub(crate) async fn open_hosted_preview(
    app: AppHandle,
    window: WebviewWindow,
) -> Result<(), String> {
    if !preview_allowed(cfg!(debug_assertions), window.label()) {
        return Err("Hosted preview is available only from the development launcher.".into());
    }
    if let Some(preview) = app.get_webview_window(PREVIEW_WINDOW) {
        preview
            .show()
            .map_err(|_| "Cannot show the preview window.")?;
        preview
            .set_focus()
            .map_err(|_| "Cannot focus the preview window.")?;
        return Ok(());
    }
    let ready = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|_| "Cannot prepare the local preview check.")?
        .get(PREVIEW_URL)
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false);
    if !ready {
        return Err("Start the local web preview on 127.0.0.1:1441 first.".into());
    }
    let url = Url::parse(PREVIEW_URL).map_err(|_| "Invalid local preview address.")?;
    // This label has no desktop capability; the demo never receives native authority.
    WebviewWindowBuilder::new(&app, PREVIEW_WINDOW, WebviewUrl::External(url))
        .title("Relay Hosted — DEMO")
        .inner_size(1280.0, 900.0)
        .min_inner_size(360.0, 560.0)
        .center()
        .incognito(true)
        .on_navigation(navigation_allowed)
        .on_new_window(|_, _| NewWindowResponse::Deny)
        .build()
        .map_err(|_| "Cannot open the local preview window.")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_requires_debug_and_main_caller() {
        assert!(preview_allowed(true, "main"));
        assert!(!preview_allowed(false, "main"));
        assert!(!preview_allowed(true, PREVIEW_WINDOW));
        assert!(!preview_allowed(true, "codex-sign-in"));
    }

    #[test]
    fn preview_navigation_is_fixed_to_one_loopback_page() {
        assert!(navigation_allowed(&Url::parse(PREVIEW_URL).unwrap()));
        assert!(navigation_allowed(
            &Url::parse(&format!("{PREVIEW_URL}?page=usage")).unwrap()
        ));
        for forbidden in [
            "https://example.test/preview.html",
            "http://localhost:1441/preview.html",
            "http://127.0.0.1:1420/",
            "http://127.0.0.1:1441/",
            "http://user:pass@127.0.0.1:1441/preview.html",
            "file:///C:/private.txt",
            "http://127.0.0.1:1441/preview.html/other",
        ] {
            assert!(
                !navigation_allowed(&Url::parse(forbidden).unwrap()),
                "{forbidden}"
            );
        }
    }

    #[test]
    fn preview_has_no_default_desktop_capability() {
        let capability: serde_json::Value =
            serde_json::from_str(include_str!("../../capabilities/default.json")).unwrap();
        assert_eq!(capability["windows"], serde_json::json!(["main"]));
        assert!(capability.get("remote").is_none());
    }
}

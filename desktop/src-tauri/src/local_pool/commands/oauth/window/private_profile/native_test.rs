//! Explicit local WebView2 acceptance; no provider or installed Relay is used.
use super::PrivateProfile;
use crate::local_pool::commands::oauth::window::{
    destroy_previous_sign_in_window, sign_in_window_config,
};
use serde_json::Value;
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, WebviewWindow, WebviewWindowBuilder, webview::PageLoadEvent};
use tokio::sync::oneshot;
use url::Url;

#[test]
#[ignore = "requires a local Windows desktop and WebView2 runtime"]
fn private_webviews_start_clean_while_another_private_window_stays_open() {
    let root = crate::files::test_temp_root("private-webview-native");
    let outcome = Arc::new(Mutex::new(None));
    let result = outcome.clone();
    let test_root = root.clone();
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    context.config_mut().identifier = "com.zenith.relay.private-sign-in-test".into();
    let app = tauri::Builder::default()
        .any_thread()
        .build(context)
        .unwrap();
    app.run_return(move |app, event| {
        if let tauri::RunEvent::ExitRequested {
            api, code: None, ..
        } = &event
        {
            api.prevent_exit();
        }
        if matches!(event, tauri::RunEvent::Ready) {
            let app = app.clone();
            let result = result.clone();
            let root = test_root.clone();
            tauri::async_runtime::spawn(async move {
                let checked = tokio::time::timeout(Duration::from_secs(40), check(&app, &root))
                    .await
                    .map_err(|_| "native private-window check timed out".to_owned())
                    .and_then(|value| value);
                *result.lock().unwrap() = Some(checked);
                app.exit(0);
            });
        }
    });
    let checked = outcome.lock().unwrap().take().expect("native check ran");
    checked.unwrap();
    let sessions = root.join("sign-in-sessions");
    assert!(std::fs::read_dir(sessions).unwrap().next().is_none());
    std::fs::remove_dir_all(root).unwrap();
}

async fn check(app: &AppHandle, root: &Path) -> Result<(), String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| error.to_string())?;
    let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let router = axum::Router::new().fallback(|| async {
        axum::response::Html(
            "<!doctype html><meta charset=utf-8><title>Private window test</title>",
        )
    });
    tokio::select! {
        result = exercise(app, root, url) => result,
        result = async { axum::serve(listener, router).await } => {
            Err(format!("synthetic HTTP server ended: {result:?}"))
        }
    }
}

async fn exercise(app: &AppHandle, root: &Path, url: Url) -> Result<(), String> {
    let preview_profile = PrivateProfile::create(root).map_err(|error| error.to_string())?;
    let preview = open(app, url.clone(), &preview_profile, Some("private-preview")).await?;
    preview_profile.attach(&preview);
    let first_profile = PrivateProfile::create(root).map_err(|error| error.to_string())?;
    let first = open(app, url.clone(), &first_profile, None).await?;
    first_profile.attach(&first);
    let written = evaluate(&first, "document.cookie='relay_isolation=first; Path=/'; localStorage.setItem('relay-isolation', 'first'); ({cookie:document.cookie,stored:localStorage.getItem('relay-isolation')})").await?;
    if written["stored"] != "first" || written["cookie"] != "relay_isolation=first" {
        return Err("synthetic browser state was not written".into());
    }
    destroy_previous_sign_in_window(app)
        .await
        .map_err(|error| format!("{error:?}"))?;
    drop(first);
    let second_profile = PrivateProfile::create(root).map_err(|error| error.to_string())?;
    let second = open(app, url, &second_profile, None).await?;
    second_profile.attach(&second);
    for window in [&preview, &second] {
        let observed = evaluate(
            window,
            "({cookie:document.cookie,stored:localStorage.getItem('relay-isolation')})",
        )
        .await?;
        if observed["cookie"] != "" || !observed["stored"].is_null() {
            return Err("browser state crossed a private-window boundary".into());
        }
    }
    second.destroy().map_err(|error| error.to_string())?;
    preview.destroy().map_err(|error| error.to_string())?;
    // Keep the UI event loop alive while native windows and processes shut down.
    tokio::time::timeout(Duration::from_secs(12), async {
        while std::fs::read_dir(root.join("sign-in-sessions"))
            .unwrap()
            .next()
            .is_some()
        {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .map_err(|_| "native profiles were not cleaned after browser exit")?;
    tokio::time::sleep(Duration::from_millis(500)).await;
    if std::fs::read_dir(root.join("sign-in-sessions"))
        .unwrap()
        .next()
        .is_some()
    {
        return Err("browser shutdown recreated a cleaned profile".into());
    }
    Ok(())
}

async fn open(
    app: &AppHandle,
    url: Url,
    profile: &PrivateProfile,
    label: Option<&str>,
) -> Result<WebviewWindow, String> {
    let mut config = sign_in_window_config(url);
    if let Some(label) = label {
        config.label = label.into();
    }
    let (sender, receiver) = oneshot::channel();
    let ready = Mutex::new(Some(sender));
    let window = WebviewWindowBuilder::from_config(app, &config)
        .map_err(|error| error.to_string())?
        .visible(false)
        .data_directory(profile.path().to_path_buf())
        .on_page_load(move |_, payload| {
            if matches!(payload.event(), PageLoadEvent::Finished)
                && let Some(sender) = ready.lock().unwrap().take()
            {
                let _ = sender.send(());
            }
        })
        .build()
        .map_err(|error| error.to_string())?;
    receiver.await.map_err(|error| error.to_string())?;
    Ok(window)
}

async fn evaluate(window: &WebviewWindow, script: &str) -> Result<Value, String> {
    let (sender, receiver) = oneshot::channel();
    let sender = Mutex::new(Some(sender));
    window
        .eval_with_callback(script, move |value| {
            if let Some(sender) = sender.lock().unwrap().take() {
                let _ = sender.send(value);
            }
        })
        .map_err(|error| error.to_string())?;
    let raw = receiver.await.map_err(|error| error.to_string())?;
    serde_json::from_str(&raw).map_err(|error| error.to_string())
}

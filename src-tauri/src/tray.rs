use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WebviewWindow, WebviewWindowBuilder,
};

use crate::local_pool::{commands::gateway, DesktopState};
use crate::platform::ui_text;

pub(crate) const MAIN_WINDOW_LABEL: &str = "main";

const TRAY_ID: &str = "zenith-relay";

pub struct AppState {
    allow_exit: Mutex<bool>,
    opening_main_window: AtomicBool,
}

struct TrayUi {
    status: MenuItem<tauri::Wry>,
    toggle: MenuItem<tauri::Wry>,
    tray: TrayIcon<tauri::Wry>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            allow_exit: Mutex::new(false),
            opening_main_window: AtomicBool::new(false),
        }
    }

    pub fn request_exit(&self) {
        if let Ok(mut allow_exit) = self.allow_exit.lock() {
            *allow_exit = true;
        }
    }

    pub fn should_prevent_exit(&self) -> bool {
        self.allow_exit
            .lock()
            .map(|allow_exit| !*allow_exit)
            .unwrap_or(true)
    }

    fn try_start_main_window_open(&self) -> bool {
        self.opening_main_window
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn finish_main_window_open(&self) {
        self.opening_main_window.store(false, Ordering::Release);
    }
}

pub fn build_tray(app: &AppHandle, _state: &State<AppState>) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "status", status_text(false, 0), false, None::<&str>)?;
    let show = MenuItem::with_id(
        app,
        "show",
        ui_text("Open Zenith Relay", "Открыть Zenith Relay"),
        true,
        None::<&str>,
    )?;
    let toggle = MenuItem::with_id(app, "toggle", toggle_text(false), true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", ui_text("Quit", "Выйти"), true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&status, &show, &toggle, &separator, &quit])?;

    let icon = Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
    let tray = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip(tooltip_text(false))
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            "toggle" => {
                let tray = app.state::<TrayUi>();
                let _ = tray.toggle.set_enabled(false);
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    toggle_pool(app).await;
                });
            }
            "quit" => {
                app.state::<AppState>().request_exit();
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button,
                button_state,
                ..
            } = event
            {
                match (button, button_state) {
                    (MouseButton::Left, MouseButtonState::Up) => {
                        show_main_window(tray.app_handle());
                    }
                    (MouseButton::Right, MouseButtonState::Down) => {
                        let app = tray.app_handle().clone();
                        tauri::async_runtime::spawn(async move {
                            refresh_tray(&app).await;
                        });
                    }
                    _ => {}
                }
            }
        })
        .build(app)?;

    app.manage(TrayUi {
        status,
        toggle,
        tray,
    });

    Ok(())
}

pub async fn refresh_tray(app: &AppHandle) {
    let runtime = app.state::<DesktopState>().gateway.runtime().await;
    let running = runtime.is_some();
    let participants = runtime
        .map(|runtime| runtime.candidate_runtime_order().len())
        .unwrap_or_default();
    let ui = app.state::<TrayUi>();
    let _ = ui.status.set_text(status_text(running, participants));
    let _ = ui.toggle.set_text(toggle_text(running));
    let _ = ui.toggle.set_enabled(true);
    let _ = ui.tray.set_tooltip(Some(tooltip_text(running)));
}

async fn toggle_pool(app: AppHandle) {
    let running = app
        .state::<DesktopState>()
        .gateway
        .runtime()
        .await
        .is_some();
    let result = if running {
        gateway::stop_local_gateway(app.clone(), app.state()).await
    } else {
        gateway::start_local_gateway(app.clone(), app.state()).await
    };
    let _ = app.emit("zenith-state-changed", ());
    if result.is_err() {
        let ui = app.state::<TrayUi>();
        let _ = ui.status.set_text(ui_text(
            "Could not change pool state",
            "Не удалось изменить состояние пула",
        ));
        let _ = ui.toggle.set_enabled(true);
        show_main_window(&app);
    }
}

fn status_text(running: bool, participants: usize) -> String {
    if running {
        format!(
            "{} {participants}",
            ui_text("Pool running · members:", "Пул работает · участников:")
        )
    } else {
        ui_text("Pool stopped", "Пул остановлен").to_string()
    }
}

fn toggle_text(running: bool) -> &'static str {
    if running {
        ui_text("Stop pool", "Остановить пул")
    } else {
        ui_text("Start pool", "Запустить пул")
    }
}

fn tooltip_text(running: bool) -> String {
    format!(
        "Zenith Relay · {}",
        if running {
            ui_text("pool running", "пул работает")
        } else {
            ui_text("pool stopped", "пул остановлен")
        }
    )
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        if let Some(state) = app.try_state::<DesktopState>() {
            state.set_background_session_active(true);
        }
        reveal_main_window(&window);
        return;
    }

    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if !state.try_start_main_window_open() {
        return;
    }

    // WebView2 window creation from a synchronous tray or single-instance
    // handler can deadlock on Windows. Run the creation after that handler
    // returns and coalesce repeated clicks while it is in flight.
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
            reveal_main_window(&window);
        } else {
            // The frontend reveals a new window after applying its saved theme.
            let _ = create_main_window(&app);
        }
        if let Some(state) = app.try_state::<AppState>() {
            state.finish_main_window_open();
        }
    });
}

fn reveal_main_window(window: &WebviewWindow<tauri::Wry>) {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

/// Creates the primary webview with the persistent data directory used by the
/// initial application window. The native app, tray, and local gateway remain
/// alive when this window is later destroyed.
pub fn create_main_window(app: &AppHandle) -> tauri::Result<WebviewWindow<tauri::Wry>> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == MAIN_WINDOW_LABEL)
        .ok_or_else(|| std::io::Error::other("main window configuration is missing"))?;
    let webview_data = crate::platform::webview_data_dir(app).map_err(std::io::Error::other)?;
    let window = WebviewWindowBuilder::from_config(app, config)?
        .data_directory(webview_data)
        .visible(false)
        .build()?;
    if let Some(state) = app.try_state::<DesktopState>() {
        state.set_background_session_active(true);
    }
    Ok(window)
}

/// Releases the WebView renderer while preserving the native process, tray,
/// managed state, and local gateway. Opening Relay from the tray recreates it.
pub fn close_main_window(app: &AppHandle) {
    if let Some(state) = app.try_state::<DesktopState>() {
        state.set_background_session_active(false);
    }
    if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
        let _ = window.destroy();
    }
}

pub(crate) fn is_main_window_label(label: &str) -> bool {
    label == MAIN_WINDOW_LABEL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_status_counts_only_running_pool_members() {
        assert!(status_text(true, 3).ends_with('3'));
        assert!(!status_text(false, 3).contains('3'));
    }

    #[test]
    fn main_window_opening_is_single_flight() {
        let state = AppState::new();

        assert!(state.try_start_main_window_open());
        assert!(!state.try_start_main_window_open());
        state.finish_main_window_open();
        assert!(state.try_start_main_window_open());
    }

    #[test]
    fn main_window_lifecycle_does_not_match_auxiliary_windows() {
        assert!(is_main_window_label(MAIN_WINDOW_LABEL));
        assert!(!is_main_window_label("oauth"));
    }

    #[test]
    fn runtime_stays_alive_until_the_user_explicitly_quits() {
        let state = AppState::new();

        assert!(state.should_prevent_exit());
        state.request_exit();
        assert!(!state.should_prevent_exit());
    }
}

use std::sync::Mutex;

use tauri::{
    image::Image,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager, State, WebviewWindowBuilder,
};

use crate::codex_config::reset_provider;
use crate::platform::ui_text;

pub struct AppState {
    allow_exit: Mutex<bool>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            allow_exit: Mutex::new(false),
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
}

pub fn build_tray(app: &AppHandle, _state: &State<AppState>) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", ui_text("Show", "Показать"), true, None::<&str>)?;
    let reset = MenuItem::with_id(
        app,
        "reset",
        ui_text("Reset settings", "Сбросить настройки"),
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", ui_text("Quit", "Выйти"), true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &reset, &quit])?;

    let icon = Image::from_bytes(include_bytes!("../icons/32x32.png"))?;
    TrayIconBuilder::new()
        .tooltip("Zenith Codex")
        .icon(icon)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            "reset" => {
                let _ = reset_provider();
                let _ = app.emit("zenith-state-changed", ());
                show_main_window(app);
            }
            "quit" => {
                app.state::<AppState>().request_exit();
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;

    Ok(())
}

pub fn show_main_window(app: &AppHandle) {
    let window = if let Some(window) = app.get_webview_window("main") {
        window
    } else {
        let Some(config) = app
            .config()
            .app
            .windows
            .iter()
            .find(|window| window.label == "main")
        else {
            return;
        };
        match WebviewWindowBuilder::from_config(app, config).and_then(|builder| builder.build()) {
            Ok(window) => window,
            Err(_) => return,
        }
    };
    let _ = window.show();
    let _ = window.set_focus();
}

pub fn close_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.destroy();
    }
}

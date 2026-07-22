mod accounts;
pub(crate) mod background;
pub mod commands;
mod error;
mod host;
mod models;
mod profiles;
mod remote;
mod state;
mod store;

use std::{fs, path::Path, time::Instant};

pub use state::DesktopState;

pub fn initialize(app: &tauri::AppHandle) -> error::Result<DesktopState> {
    let started = Instant::now();
    let root = crate::platform::relay_dir(app)
        .map_err(|message| error::LocalPoolError::new(error::ErrorCode::Io, message))?;
    create_storage_directory(&root)?;
    let directory_ready = started.elapsed();
    store::secret_store::initialize(&root.join("data"))?;
    let secrets_ready = started.elapsed();
    let state = DesktopState::open(root)?;
    let state_ready = started.elapsed();
    state.set_app_handle(app.clone());
    if cfg!(debug_assertions) {
        eprintln!(
            "[startup] storage_dir={}ms secret_store={}ms local_state={}ms total={}ms",
            directory_ready.as_millis(),
            (secrets_ready - directory_ready).as_millis(),
            (state_ready - secrets_ready).as_millis(),
            state_ready.as_millis(),
        );
    }
    Ok(state)
}

fn create_storage_directory(path: &Path) -> error::Result<()> {
    fs::create_dir_all(path).map_err(|error| {
        layout_error(format!(
            "failed to create relay data directory {}: {error}",
            path.display()
        ))
    })?;
    let metadata = fs::symlink_metadata(path).map_err(|error| layout_error(error.to_string()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(layout_error(format!(
            "relay data path must be a real directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn layout_error(message: impl Into<String>) -> error::LocalPoolError {
    error::LocalPoolError::new(error::ErrorCode::Io, message)
}

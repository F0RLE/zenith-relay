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

pub use state::DesktopState;

pub fn initialize(app: &tauri::AppHandle) -> error::Result<DesktopState> {
    let root = crate::platform::local_pool_dir(app)
        .map_err(|message| error::LocalPoolError::new(error::ErrorCode::Io, message))?;
    let state = DesktopState::open(root)?;
    state.set_app_handle(app.clone());
    Ok(state)
}

pub mod commands;
mod error;
mod models;
mod store;

use crate::platform;
use error::Result;
use models::{LocalPoolSnapshot, RuntimeTarget};
use store::LocalPoolStore;
use tauri::AppHandle;

pub fn snapshot(app: &AppHandle) -> Result<LocalPoolSnapshot> {
    let store = LocalPoolStore::open(
        platform::local_pool_dir(app)
            .map_err(|message| error::LocalPoolError::new(error::ErrorCode::Io, message))?,
    )?;
    Ok(LocalPoolSnapshot {
        schema_version: store.metadata().schema_version,
        runtime_target: RuntimeTarget {
            kind: "local",
            connected: store.gateway().enabled,
        },
        gateway: store.gateway().clone(),
        platform: platform::platform_name(),
        capabilities: platform::capabilities(),
        warnings: Vec::new(),
    })
}

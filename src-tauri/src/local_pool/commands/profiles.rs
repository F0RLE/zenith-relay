use crate::{
    local_pool::{
        error::{CommandError, ErrorCode, LocalPoolError},
        profiles::codex,
        state::DesktopState,
        store::secret_store,
    },
    platform::default_codex_home,
};
use tauri::State;

#[tauri::command]
pub async fn attach_codex_to_local_gateway(
    key_id: String,
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    let (key, port) = {
        let store = state.store()?;
        let key = store
            .key(&key_id)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local key not found"))?;
        (key, store.gateway().port)
    };
    if !key.enabled || !super::pool::has_usable_source(&state, &key)? {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "local key is not available for any enabled source",
        )
        .into());
    }
    let secret = secret_store::load(&key.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local key secret is missing"))?;
    codex::attach(
        &default_codex_home(),
        &state.profile_backup_root(),
        &format!("http://127.0.0.1:{port}/v1"),
        &secret,
    )
    .map_err(Into::into)
}

#[tauri::command]
pub async fn restore_codex_profile(state: State<'_, DesktopState>) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    codex::restore(&default_codex_home(), &state.profile_backup_root()).map_err(Into::into)
}

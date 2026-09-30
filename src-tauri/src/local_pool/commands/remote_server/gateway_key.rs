use super::session::{active_client, remote_error};
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    state::DesktopState,
};
use tauri::State;

#[tauri::command]
pub async fn reveal_remote_gateway_api_key(
    state: State<'_, DesktopState>,
) -> Result<String, CommandError> {
    let _mutation = state.setup_guard().await;
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    Ok(client
        .profile_credential()
        .await
        .map_err(remote_error)?
        .secret)
}

#[tauri::command]
pub async fn rotate_remote_gateway_api_key(
    state: State<'_, DesktopState>,
) -> Result<String, CommandError> {
    let _mutation = state.setup_guard().await;
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    let rotation = client
        .prepare_profile_key_rotation()
        .await
        .map_err(remote_error)?;
    if let Err(error) = client
        .commit_profile_key_rotation(&rotation.rotation_id)
        .await
    {
        let committed = client.profile_credential().await.is_ok_and(|credential| {
            credential.key_id == rotation.key_id
                && credential.base_url == rotation.base_url
                && credential.secret == rotation.secret
        });
        if !committed {
            let _ = client
                .abort_profile_key_rotation(&rotation.rotation_id)
                .await;
            return Err(remote_error(error));
        }
    }
    Ok(rotation.secret)
}

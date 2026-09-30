use super::super::pool::write_configuration_preset;
use super::session::{active_client, remote_error};
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    remote::client::RemoteClient,
    state::DesktopState,
};
use std::fs;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use zenith_relay_core::protocol::{
    ConfigurationPreset, ConfigurationPresetApplyInput, ConfigurationPresetApplyResult,
    ConfigurationPresetPreview, ConfigurationPresetPreviewInput,
};

const MAX_CONFIGURATION_PRESET_BYTES: usize = 1024 * 1024;

#[tauri::command]
pub async fn export_remote_configuration_preset(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<Option<String>, CommandError> {
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    let document = client.configuration_preset().await.map_err(remote_error)?;
    write_configuration_preset(&document.preset, &app)
}

#[tauri::command]
pub async fn preview_remote_configuration_preset(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<Option<ConfigurationPresetPreview>, CommandError> {
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("Zenith Relay configuration", &["json"])
        .blocking_pick_file()
    else {
        return Ok(None);
    };
    let path = path.into_path().map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "selected preset path is invalid")
    })?;
    let content = fs::read(&path).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "configuration preset could not be read",
        )
    })?;
    if content.len() > MAX_CONFIGURATION_PRESET_BYTES {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "configuration preset exceeds 1 MiB",
        )
        .into());
    }
    let preset: ConfigurationPreset = serde_json::from_slice(&content).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "configuration preset is invalid or contains unsupported fields",
        )
    })?;
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    require_preset_protocol_contract(&client, &preset).await?;
    client
        .preview_configuration_preset(&ConfigurationPresetPreviewInput { preset })
        .await
        .map(Some)
        .map_err(remote_error)
}

#[tauri::command]
pub async fn apply_remote_configuration_preset(
    input: ConfigurationPresetApplyInput,
    state: State<'_, DesktopState>,
) -> Result<ConfigurationPresetApplyResult, CommandError> {
    let _mutation = state.setup_guard().await;
    let Some((_, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    require_preset_protocol_contract(&client, &input.preset).await?;
    client
        .apply_configuration_preset(&input)
        .await
        .map_err(remote_error)
}

async fn require_preset_protocol_contract(
    client: &RemoteClient,
    preset: &ConfigurationPreset,
) -> Result<(), CommandError> {
    let needs_contract = preset.schema_version >= 4
        || preset.settings.sources.iter().any(|source| {
            source.protocol_bindings.iter().any(|binding| {
                !matches!(
                    binding.adapter,
                    zenith_relay_core::SourceAdapter::Native
                        | zenith_relay_core::SourceAdapter::ResponsesToMessages
                        | zenith_relay_core::SourceAdapter::ResponsesToGemini
                )
            })
        });
    let needs_tool_policy =
        preset.schema_version >= 5 || preset.settings.routing.tool_policy.is_some();
    let needs_rotation = preset.schema_version >= 6
        || preset
            .settings
            .routing
            .pool_routing
            .as_ref()
            .is_some_and(zenith_relay_core::PoolRoutingPolicy::is_current_rotation);
    let capabilities = client.capabilities().await.map_err(remote_error)?;
    for (needed, feature) in [
        (
            needs_contract,
            zenith_relay_core::protocol::Feature::SourceProtocols,
        ),
        (
            needs_tool_policy,
            zenith_relay_core::protocol::Feature::ToolPolicy,
        ),
        (
            needs_rotation,
            zenith_relay_core::protocol::Feature::Rotation,
        ),
    ] {
        if needed && !capabilities.supports(feature) {
            return Err(LocalPoolError::new(
                ErrorCode::UnsupportedSchema,
                "server does not support this configuration preset protocol version",
            )
            .into());
        }
    }
    Ok(())
}

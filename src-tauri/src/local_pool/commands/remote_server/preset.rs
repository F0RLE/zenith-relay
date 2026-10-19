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
    Capabilities, ConfigurationPreset, ConfigurationPresetApplyInput,
    ConfigurationPresetApplyResult, ConfigurationPresetPreview, ConfigurationPresetPreviewInput,
};
use zenith_relay_core::PoolRoutingPolicy;

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
    let needs_rotation =
        preset.schema_version >= 6 || preset.settings.routing.pool_routing.is_some();
    let capabilities = client.capabilities().await.map_err(remote_error)?;
    if needs_rotation
        && !supports_preset_rotation(&capabilities, preset.settings.routing.pool_routing.as_ref())
    {
        return Err(LocalPoolError::new(
            ErrorCode::UnsupportedSchema,
            "server does not support this configuration preset rotation version",
        )
        .into());
    }
    for (needed, feature) in [
        (
            needs_contract,
            zenith_relay_core::protocol::Feature::SourceProtocols,
        ),
        (
            needs_tool_policy,
            zenith_relay_core::protocol::Feature::ToolPolicy,
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

fn supports_preset_rotation(
    capabilities: &Capabilities,
    policy: Option<&PoolRoutingPolicy>,
) -> bool {
    let version = match policy {
        Some(policy) => {
            if policy.validate().is_err() {
                return false;
            }
            policy.version.max(2)
        }
        None => 2,
    };
    // Import migrates known legacy policies; direct policy updates remain version-exact.
    capabilities.supports_rotation_policy(u64::from(version))
        || (version == 2 && capabilities.supports_rotation_policy(3))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zenith_relay_core::{PoolMemberKind, PoolRoutingMember, PoolRoutingMode};

    fn servers() -> [Capabilities; 2] {
        let current = Capabilities::personal_server("server", "fingerprint");
        let mut older = current.clone();
        older.protocol_version = 2;
        older.compatibility_min_client = 2;
        older.features.remove("rotation_v3");
        older.features.insert("rotation_v2".into());
        [older, current]
    }

    fn policy(version: u8, mode: PoolRoutingMode) -> PoolRoutingPolicy {
        PoolRoutingPolicy {
            version,
            mode,
            members: vec![PoolRoutingMember {
                kind: PoolMemberKind::Account,
                id: "account".into(),
                weight: 7,
                max_concurrency: 4,
            }],
        }
    }

    #[test]
    fn legacy_rotation_presets_import_on_both_server_generations_without_rewriting_them() {
        for server in servers() {
            assert!(supports_preset_rotation(&server, None));
            for (version, mode) in [
                (1, PoolRoutingMode::Smart),
                (1, PoolRoutingMode::InOrder),
                (1, PoolRoutingMode::RoundRobin),
                (2, PoolRoutingMode::Automatic),
                (2, PoolRoutingMode::InOrder),
                (2, PoolRoutingMode::RoundRobin),
            ] {
                let policy = policy(version, mode);
                let original = policy.clone();
                assert!(supports_preset_rotation(&server, Some(&policy)));
                assert_eq!(policy, original);
            }
        }
    }

    #[test]
    fn current_presets_require_a_current_server_and_direct_updates_stay_version_exact() {
        let [older, current] = servers();
        for mode in [
            PoolRoutingMode::Automatic,
            PoolRoutingMode::InOrder,
            PoolRoutingMode::RoundRobin,
            PoolRoutingMode::WeightedRoundRobin,
            PoolRoutingMode::Failover,
        ] {
            let policy = policy(3, mode);
            assert!(supports_preset_rotation(&current, Some(&policy)));
            assert!(!supports_preset_rotation(&older, Some(&policy)));
        }
        assert!(!current.supports_rotation_policy(2));
        assert!(!older.supports_rotation_policy(3));
    }

    #[test]
    fn invalid_policies_and_missing_rotation_capabilities_are_not_accepted() {
        for server in servers() {
            for (version, mode) in [
                (0, PoolRoutingMode::Automatic),
                (1, PoolRoutingMode::Automatic),
                (2, PoolRoutingMode::Smart),
                (2, PoolRoutingMode::WeightedRoundRobin),
                (2, PoolRoutingMode::Failover),
                (3, PoolRoutingMode::Smart),
                (4, PoolRoutingMode::Automatic),
            ] {
                assert!(!supports_preset_rotation(
                    &server,
                    Some(&policy(version, mode))
                ));
            }
            let mut invalid = policy(2, PoolRoutingMode::RoundRobin);
            invalid.members[0].weight = 0;
            assert!(!supports_preset_rotation(&server, Some(&invalid)));
        }
        let mut unsupported = servers()[1].clone();
        unsupported.features.remove("rotation_v3");
        assert!(!supports_preset_rotation(&unsupported, None));
        assert!(!supports_preset_rotation(
            &unsupported,
            Some(&policy(2, PoolRoutingMode::RoundRobin))
        ));
    }
}

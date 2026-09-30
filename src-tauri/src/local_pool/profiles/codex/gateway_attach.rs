use super::*;

pub(crate) fn attach_ready_api(codex_home: &Path, backup_root: &Path, api_key: &str) -> Result<()> {
    attach_ready_api_with_intent(codex_home, backup_root, api_key, false)
}

pub(crate) fn attach_ready_api_explicit(
    codex_home: &Path,
    backup_root: &Path,
    api_key: &str,
) -> Result<()> {
    attach_ready_api_with_intent(codex_home, backup_root, api_key, true)
}

pub(super) fn attach_ready_api_with_intent(
    codex_home: &Path,
    backup_root: &Path,
    api_key: &str,
    rebase_newer_login: bool,
) -> Result<()> {
    switch_to_local_with(
        codex_home,
        backup_root,
        "ready_api",
        "https://api.zenithmarket.dev/v1",
        api_key,
        LocalAttachOptions {
            provider_id: READY_API_PROVIDER_ID,
            rebase_newer_login,
            ..LocalAttachOptions::default()
        },
        &OsSecretBackend,
    )
    .map(|_| ())
}

pub(crate) fn restore_ready_api(codex_home: &Path, backup_root: &Path) -> Result<bool> {
    let _profile_guard = lock_codex_profile();
    let Some(backup) = local_backup(codex_home, backup_root)? else {
        return Ok(false);
    };
    if backup.managed_provider_id != READY_API_PROVIDER_ID {
        return Ok(false);
    }
    local::restore_local_locked(codex_home, backup_root, &OsSecretBackend)?;
    Ok(true)
}

pub fn attach(
    codex_home: &Path,
    backup_root: &Path,
    key_id: &str,
    base_url: &str,
    local_key: &str,
) -> Result<ProfileBinding> {
    switch_to_local_with(
        codex_home,
        backup_root,
        key_id,
        base_url,
        local_key,
        LocalAttachOptions::default(),
        &OsSecretBackend,
    )
}

pub fn attach_with_catalog(
    codex_home: &Path,
    backup_root: &Path,
    key_id: &str,
    base_url: &str,
    local_key: &str,
    catalog_json: &str,
) -> Result<ProfileBinding> {
    switch_to_local_with(
        codex_home,
        backup_root,
        key_id,
        base_url,
        local_key,
        LocalAttachOptions {
            catalog_json: Some(catalog_json),
            rebase_newer_login: true,
            ..LocalAttachOptions::default()
        },
        &OsSecretBackend,
    )
}

pub fn attach_with_catalog_and_websockets(
    codex_home: &Path,
    backup_root: &Path,
    key_id: &str,
    base_url: &str,
    local_key: &str,
    catalog_json: &str,
    supports_websockets: bool,
) -> Result<ProfileBinding> {
    switch_to_local_with(
        codex_home,
        backup_root,
        key_id,
        base_url,
        local_key,
        LocalAttachOptions {
            catalog_json: Some(catalog_json),
            supports_websockets,
            rebase_newer_login: true,
            ..LocalAttachOptions::default()
        },
        &OsSecretBackend,
    )
}

#[cfg(test)]
pub(crate) fn direct_source_model_catalog(
    codex_home: &Path,
    source_models: &[String],
) -> Result<Option<String>> {
    catalog::direct_source_model_catalog_with_manifest(codex_home, source_models, None)
}

pub(crate) fn direct_source_model_catalog_with_capabilities(
    codex_home: &Path,
    source_models: &[String],
    metadata: &ModelMetadataCatalog,
) -> Result<Option<String>> {
    catalog::direct_source_model_catalog_with_capabilities(codex_home, source_models, metadata)
}

#[cfg(test)]
pub(crate) fn direct_source_model_catalog_with_manifest(
    codex_home: &Path,
    source_models: &[String],
    source_manifest: Option<&Value>,
) -> Result<Option<String>> {
    catalog::direct_source_model_catalog_with_manifest(codex_home, source_models, source_manifest)
}

pub(crate) fn attach_with_oauth_and_options(
    codex_home: &Path,
    backup_root: &Path,
    key_id: &str,
    base_url: &str,
    local_key: &str,
    options: OAuthAttachOptions<'_>,
) -> Result<ProfileBinding> {
    switch_to_local_with(
        codex_home,
        backup_root,
        key_id,
        base_url,
        local_key,
        LocalAttachOptions {
            bound_oauth: Some(options.bound_oauth),
            catalog_json: Some(options.catalog_json),
            supports_websockets: options.supports_websockets,
            rebase_newer_login: true,
            ..LocalAttachOptions::default()
        },
        &OsSecretBackend,
    )
}

pub fn restore(codex_home: &Path, backup_root: &Path) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    ensure_single_profile_backup(codex_home, backup_root)?;
    if account_backup_for_profile(codex_home, backup_root)?.is_some() {
        account::restore_account_locked(codex_home, backup_root, &OsSecretBackend)?;
        return Ok(());
    }
    local::restore_local_locked(codex_home, backup_root, &OsSecretBackend)
}

/// Updates the managed profile and returns the previous provider setting when
/// the profile was managed. Callers that persist a second copy of this state
/// can use the returned value to restore the profile if that later write fails.
pub fn set_local_gateway_websockets_with_previous(
    codex_home: &Path,
    backup_root: &Path,
    enabled: bool,
    expected_credential_id: Option<&str>,
) -> Result<Option<bool>> {
    set_local_gateway_websockets_with_backend(
        codex_home,
        backup_root,
        enabled,
        expected_credential_id,
        &OsSecretBackend,
    )
}

pub(super) fn set_local_gateway_websockets_with_backend(
    codex_home: &Path,
    backup_root: &Path,
    enabled: bool,
    expected_credential_id: Option<&str>,
    secrets: &impl SecretBackend,
) -> Result<Option<bool>> {
    let _profile_guard = lock_codex_profile();
    // A setting-only request must be a no-op when Codex has never created a
    // profile. `switch_transaction::run` correctly journals mutations, but it
    // also creates its target directory before calling this closure.
    if !codex_home.exists() {
        return Ok(None);
    }
    switch_transaction::run(codex_home, secrets, |secrets| {
        let profile_dir = canonical_profile_dir(codex_home)?;
        let config_path = profile_dir.join(CONFIG_FILE);
        let backup_path = backup_path(backup_root);
        let original_config = read_optional_bytes(&config_path)?;
        let Some(config_text) = snapshot_text(&original_config, &config_path)? else {
            return Ok(None);
        };
        let original_backup = read_optional_bytes(&backup_path)?;
        let Some(mut backup) = parse_backup_snapshot(&original_backup, &backup_path)? else {
            return Ok(None);
        };
        if expected_credential_id.is_some_and(|id| {
            backup.credential_kind() != ProfileCredentialKind::LocalGateway
                || backup.managed_key_id != id
        }) {
            return Ok(None);
        }
        let mut document = parse_config(config_text)?;
        if !managed_config_matches(&document, &backup) {
            return Ok(None);
        }
        let previous_enabled = document
            .get("model_providers")
            .and_then(Item::as_table_like)
            .and_then(|providers| providers.get(&backup.managed_provider_id))
            .and_then(Item::as_table_like)
            .and_then(|provider| provider.get("supports_websockets"))
            .and_then(Item::as_bool)
            // A managed provider predates this field in some profiles. The
            // current Codex contract treats the omitted field as enabled.
            .or(Some(true));
        if document
            .get("model_providers")
            .and_then(Item::as_table_like)
            .and_then(|providers| providers.get(&backup.managed_provider_id))
            .and_then(Item::as_table_like)
            .and_then(|provider| provider.get("supports_websockets"))
            .and_then(Item::as_bool)
            == Some(enabled)
            && backup.managed_supports_websockets == Some(enabled)
        {
            return Ok(previous_enabled);
        }
        if !set_managed_websockets(&mut document, &backup.managed_provider_id, enabled) {
            return Ok(previous_enabled);
        }
        let next_config = document.to_string();
        if let Some(secret_ref) = backup.projection_secret_ref.as_deref() {
            projection::update_websockets(
                secret_ref,
                &backup.managed_provider_id,
                enabled,
                secrets,
            )?;
        }
        backup.managed_supports_websockets = Some(enabled);
        let next_backup = serialize_backup(&backup)?;
        if next_config != config_text {
            replace_if_unchanged(&config_path, &original_config, &next_config)?;
        }
        if let Err(error) = replace_if_unchanged(&backup_path, &original_backup, &next_backup) {
            let rollback = rollback_file(&config_path, &next_config, &original_config);
            return Err(with_rollback(error, rollback));
        }
        Ok(previous_enabled)
    })
}

use super::*;

pub(super) fn switch_to_local_with(
    codex_home: &Path,
    backup_root: &Path,
    key_id: &str,
    base_url: &str,
    local_key: &str,
    options: LocalAttachOptions<'_>,
    secrets: &impl SecretBackend,
) -> Result<ProfileBinding> {
    let _profile_guard = lock_codex_profile();
    ensure_single_profile_backup(codex_home, backup_root)?;
    switch_transaction::run(codex_home, secrets, |secrets| {
        let detached_account_backup = match account_backup_for_profile(codex_home, backup_root)? {
            Some(account_backup_path) if external_account_provider_took_over(codex_home)? => {
                let backup_bytes = read_optional_bytes(&account_backup_path)?;
                let backup = parse_account_backup_snapshot(&backup_bytes, &account_backup_path)?
                    .ok_or_else(|| {
                        LocalPoolError::new(
                            ErrorCode::RecoveryRequired,
                            "ChatGPT account profile backup disappeared during the switch",
                        )
                    })?;
                remove_if_unchanged(&account_backup_path, &backup_bytes)?;
                Some(backup)
            }
            Some(_) => {
                account::restore_account_locked(codex_home, backup_root, secrets)?;
                None
            }
            None => None,
        };
        local::prepare_existing_local_binding_locked(
            codex_home,
            backup_root,
            options.rebase_newer_login,
            secrets,
        )?;
        local::attach_local_locked(
            codex_home,
            backup_root,
            key_id,
            base_url,
            local_key,
            options,
            secrets,
        )?;
        if let Some(backup) = detached_account_backup {
            for secret_ref in [
                backup.previous_auth_secret_ref,
                backup.projection_secret_ref,
            ]
            .into_iter()
            .flatten()
            {
                secrets.delete(&secret_ref)?;
            }
        }
        let backup = local_backup(codex_home, backup_root)?.ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "ChatGPT local gateway profile backup is missing after attach",
            )
        })?;
        Ok(ProfileBinding {
            profile_dir: canonical_profile_dir(codex_home)?
                .to_string_lossy()
                .into_owned(),
            credential_kind: backup.credential_kind(),
            credential_id: key_id.to_string(),
            bound_oauth_account_id: backup.bound_oauth_account_id,
            active: true,
        })
    })
}

pub(super) fn switch_to_account_with(
    codex_home: &Path,
    backup_root: &Path,
    account_id: &str,
    tokens: &TokenSet,
    provider_account_id: &str,
    secrets: &impl SecretBackend,
) -> Result<ProfileBinding> {
    switch_to_account_with_intent(
        codex_home,
        backup_root,
        account_id,
        tokens,
        provider_account_id,
        false,
        secrets,
    )
}

pub(super) fn switch_to_account_with_intent(
    codex_home: &Path,
    backup_root: &Path,
    account_id: &str,
    tokens: &TokenSet,
    provider_account_id: &str,
    rebase_newer_login: bool,
    secrets: &impl SecretBackend,
) -> Result<ProfileBinding> {
    let _profile_guard = lock_codex_profile();
    ensure_single_profile_backup(codex_home, backup_root)?;
    switch_transaction::run(codex_home, secrets, |secrets| {
        if rebase_newer_login {
            if let Some(account_backup_path) = account_backup_for_profile(codex_home, backup_root)?
            {
                let backup_bytes = read_optional_bytes(&account_backup_path)?;
                let backup = parse_account_backup_snapshot(&backup_bytes, &account_backup_path)?
                    .ok_or_else(|| {
                        LocalPoolError::new(
                            ErrorCode::RecoveryRequired,
                            "ChatGPT account profile backup disappeared during activation",
                        )
                    })?;
                let profile_dir = canonical_profile_dir(codex_home)?;
                let auth_path = profile_dir.join(AUTH_FILE);
                let auth_bytes = read_optional_bytes(&auth_path)?;
                let config_path = profile_dir.join(CONFIG_FILE);
                let config_bytes = read_optional_bytes(&config_path)?;
                let config_document =
                    parse_config(snapshot_text(&config_bytes, &config_path)?.unwrap_or_default())?;
                if !account_managed_config_matches(&config_document)
                    || !account_auth_matches_snapshot(
                        &auth_bytes,
                        &auth_path,
                        &backup.managed_access_hash,
                    )?
                {
                    account::restore_account_locked(codex_home, backup_root, secrets)?;
                }
            }
        }
        if backup_path(backup_root).exists() {
            if let Some(backup) = local_backup(codex_home, backup_root)? {
                if !rebase_newer_login {
                    local::ensure_no_newer_login(codex_home, &backup)?;
                }
            }
            local::restore_local_locked(codex_home, backup_root, secrets)?;
        }
        account::attach_account_locked(
            codex_home,
            backup_root,
            account_id,
            tokens,
            provider_account_id,
            secrets,
        )
    })
}

#[cfg(test)]
pub(super) fn ensure_test_native_catalog(home: &Path) {
    let path = home.join(MODELS_CACHE_FILE);
    let has_compatible_native = fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .and_then(|catalog_document| {
            catalog_document
                .get("models")
                .and_then(Value::as_array)
                .cloned()
        })
        .is_some_and(|models| {
            models.iter().any(|model| {
                catalog::is_native_catalog_entry(model) && codex_catalog_entry_is_compatible(model)
            })
        });
    if has_compatible_native {
        return;
    }
    let mut catalog_entry = routed_codex_catalog_entry(None, "gpt-5.6-sol", 1, None);
    catalog_entry["slug"] = Value::String("gpt-5.6-sol".into());
    catalog_entry["display_name"] = Value::String("GPT-5.6 Sol".into());
    catalog_entry["description"] = Value::String("Native test model".into());
    catalog_entry["comp_hash"] = Value::String("official".into());
    catalog_entry["default_reasoning_level"] = Value::String("low".into());
    catalog_entry["supported_reasoning_levels"] = json!([
        {"effort": "low", "description": "Low"},
        {"effort": "medium", "description": "Medium"}
    ]);
    catalog_entry["input_modalities"] = json!(["text", "image"]);
    let _ = fs::write(
        path,
        serde_json::to_string_pretty(&json!({"models": [catalog_entry]})).unwrap(),
    );
}

#[cfg(test)]
pub(super) fn attach_with(
    codex_home: &Path,
    backup_root: &Path,
    base_url: &str,
    local_key: &str,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    ensure_test_native_catalog(codex_home);
    local::prepare_existing_local_binding_locked(codex_home, backup_root, false, secrets)?;
    local::attach_local_locked(
        codex_home,
        backup_root,
        "local_gateway",
        base_url,
        local_key,
        LocalAttachOptions::default(),
        secrets,
    )
}

#[cfg(test)]
pub(super) fn attach_with_catalog_for_test(
    codex_home: &Path,
    backup_root: &Path,
    base_url: &str,
    local_key: &str,
    catalog_json: &str,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    ensure_test_native_catalog(codex_home);
    local::prepare_existing_local_binding_locked(codex_home, backup_root, false, secrets)?;
    local::attach_local_locked(
        codex_home,
        backup_root,
        "local_gateway",
        base_url,
        local_key,
        LocalAttachOptions {
            catalog_json: Some(catalog_json),
            ..LocalAttachOptions::default()
        },
        secrets,
    )
}

#[cfg(test)]
pub(super) fn restore_with(
    codex_home: &Path,
    backup_root: &Path,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    local::restore_local_locked(codex_home, backup_root, secrets)
}

#[cfg(test)]
pub(super) fn attach_account_with(
    codex_home: &Path,
    backup_root: &Path,
    account_id: &str,
    tokens: &TokenSet,
    provider_account_id: &str,
    secrets: &impl SecretBackend,
) -> Result<ProfileBinding> {
    let _profile_guard = lock_codex_profile();
    account::attach_account_locked(
        codex_home,
        backup_root,
        account_id,
        tokens,
        provider_account_id,
        secrets,
    )
}

#[cfg(test)]
pub(super) fn restore_account_with(
    codex_home: &Path,
    backup_root: &Path,
    secrets: &impl SecretBackend,
) -> Result<Option<ProfileBinding>> {
    let _profile_guard = lock_codex_profile();
    account::restore_account_locked(codex_home, backup_root, secrets)
}

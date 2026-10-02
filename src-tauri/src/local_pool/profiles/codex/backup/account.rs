use super::super::*;

pub(in crate::local_pool::profiles::codex) fn attach_account_config(document: &mut DocumentMut) {
    // The login changes here. An official ChatGPT catalog stays untouched.
    // Relay and any other selected provider are removed so the account uses
    // the native ChatGPT route; a saved external catalog path is kept.
    let official = matches!(
        root_model_provider(document).as_deref(),
        None | Some("openai")
    );
    remove_managed_provider(document, PROVIDER_ID);
    if !official {
        let catalog = root_model_catalog_json(document);
        restore_config(document, PROVIDER_ID, None, catalog.as_deref());
    }
    document.remove("openai_base_url");
}

pub(in crate::local_pool::profiles::codex) fn restore_account_config(
    document: &mut DocumentMut,
    backup: &AccountProfileBackup,
) {
    let model_catalog = root_model_catalog_json(document);
    restore_config(
        document,
        PROVIDER_ID,
        backup.previous_model_provider.as_deref(),
        model_catalog.as_deref(),
    );
    if let Some(base_url) = backup.previous_openai_base_url.as_deref() {
        document["openai_base_url"] = value(base_url);
    }
}

pub(in crate::local_pool::profiles::codex) fn account_managed_config_matches(
    document: &DocumentMut,
) -> bool {
    matches!(
        root_model_provider(document).as_deref(),
        None | Some("openai")
    ) && !document_has_provider(document)
}

pub(in crate::local_pool::profiles::codex) fn account_auth_content(
    tokens: &TokenSet,
    provider_account_id: &str,
) -> Result<String> {
    let mut token_values = serde_json::Map::new();
    token_values.insert(
        "access_token".into(),
        serde_json::Value::String(tokens.access_token().to_string()),
    );
    token_values.insert(
        "account_id".into(),
        serde_json::Value::String(provider_account_id.to_string()),
    );
    token_values.insert(
        "refresh_token".into(),
        serde_json::Value::String(tokens.refresh_token().unwrap_or_default().to_string()),
    );
    if let Some(id_token) = tokens.id_token() {
        token_values.insert(
            "id_token".into(),
            serde_json::Value::String(id_token.to_string()),
        );
    }
    let last_refresh = i64::try_from(tokens.issued_at_ms())
        .ok()
        .filter(|milliseconds| *milliseconds > 0)
        .and_then(DateTime::<Utc>::from_timestamp_millis)
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(SecondsFormat::Millis, true);
    let content = serde_json::to_string_pretty(&serde_json::json!({
        "OPENAI_API_KEY": null,
        "last_refresh": last_refresh,
        "tokens": token_values,
    }))
    .map_err(LocalPoolError::invalid_state)?;
    Ok(format!("{content}\n"))
}

pub(in crate::local_pool::profiles::codex) fn auth_snapshot_json(
    snapshot: &Option<Vec<u8>>,
) -> Option<Value> {
    snapshot
        .as_deref()
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|content| serde_json::from_str(content).ok())
}

pub(in crate::local_pool::profiles::codex) fn account_auth_matches_snapshot(
    snapshot: &Option<Vec<u8>>,
    _path: &Path,
    expected_hash: &str,
) -> Result<bool> {
    let Some(value) = auth_snapshot_json(snapshot) else {
        return Ok(false);
    };
    Ok(
        auth_credential_kind(&value) == Some(ProfileCredentialKind::OAuthAccount)
            && value
                .get("tokens")
                .and_then(|tokens| tokens.get("access_token"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|token| key_hash(token.trim()) == expected_hash),
    )
}

pub(in crate::local_pool::profiles::codex) fn account_auth_matches_tokens(
    snapshot: &Option<Vec<u8>>,
    path: &Path,
    expected_tokens: &TokenSet,
    provider_account_id: &str,
) -> Result<bool> {
    let Some(content) = snapshot_text(snapshot, path)? else {
        return Ok(false);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return Ok(false);
    };
    let Some(tokens) = value.get("tokens").and_then(serde_json::Value::as_object) else {
        return Ok(false);
    };
    Ok(
        auth_credential_kind(&value) == Some(ProfileCredentialKind::OAuthAccount)
            && tokens
                .get("account_id")
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                == Some(provider_account_id.trim())
            && managed_token(tokens, "access_token") == Some(expected_tokens.access_token())
            && managed_token(tokens, "refresh_token") == expected_tokens.refresh_token()
            && match expected_tokens.id_token() {
                Some(expected) => managed_token(tokens, "id_token") == Some(expected),
                None => managed_token(tokens, "id_token").is_none(),
            },
    )
}

pub(in crate::local_pool::profiles::codex) fn account_backup_for_profile(
    codex_home: &Path,
    backup_root: &Path,
) -> Result<Option<PathBuf>> {
    if !codex_home.exists() {
        return Ok(None);
    }
    let path = account_backup_path(backup_root, &canonical_profile_dir(codex_home)?);
    Ok(path.exists().then_some(path))
}

pub(in crate::local_pool::profiles::codex) fn ensure_single_profile_backup(
    codex_home: &Path,
    backup_root: &Path,
) -> Result<()> {
    if backup_path(backup_root).exists()
        && account_backup_for_profile(codex_home, backup_root)?.is_some()
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT profile has conflicting local gateway and account backups",
        ));
    }
    Ok(())
}

pub(in crate::local_pool::profiles::codex) fn credential_kind_locked(
    codex_home: &Path,
    backup_root: &Path,
) -> Result<Option<ProfileCredentialKind>> {
    ensure_single_profile_backup(codex_home, backup_root)?;
    if account_backup_for_profile(codex_home, backup_root)?.is_some() {
        return Ok(Some(ProfileCredentialKind::OAuthAccount));
    }
    if backup_path(backup_root).exists() {
        return Ok(local_backup(codex_home, backup_root)?.map(|backup| backup.credential_kind()));
    }
    let auth_path = codex_home.join(AUTH_FILE);
    let auth = read_optional_bytes(&auth_path)?;
    let Some(content) = snapshot_text(&auth, &auth_path)? else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return Ok(None);
    };
    Ok(auth_credential_kind(&value))
}

pub(in crate::local_pool::profiles::codex) fn auth_credential_kind(
    value: &serde_json::Value,
) -> Option<ProfileCredentialKind> {
    match value.get("auth_mode").and_then(serde_json::Value::as_str) {
        Some("chatgpt") => Some(ProfileCredentialKind::OAuthAccount),
        Some("apikey") => Some(ProfileCredentialKind::ApiKey),
        Some(_) => None,
        None if value
            .get("OPENAI_API_KEY")
            .and_then(serde_json::Value::as_str)
            .is_some() =>
        {
            Some(ProfileCredentialKind::ApiKey)
        }
        None if value
            .get("tokens")
            .and_then(serde_json::Value::as_object)
            .is_some() =>
        {
            Some(ProfileCredentialKind::OAuthAccount)
        }
        None => None,
    }
}

pub(in crate::local_pool::profiles::codex) fn account_backup_path(
    backup_root: &Path,
    profile_dir: &Path,
) -> PathBuf {
    backup_root.join(format!(
        "{ACCOUNT_BACKUP_PREFIX}{}.json",
        key_hash(profile_dir.to_string_lossy().as_ref())
    ))
}

pub(in crate::local_pool::profiles::codex) fn account_backup_secret_ref(
    profile_dir: &Path,
) -> String {
    format!(
        "profile:codex:{}:previous_auth",
        key_hash(profile_dir.to_string_lossy().as_ref())
    )
}

pub(in crate::local_pool::profiles::codex) fn canonical_profile_dir(
    path: &Path,
) -> Result<PathBuf> {
    let canonical = fs::canonicalize(path).map_err(|error| io_error_at(path, error))?;
    if !canonical.is_dir() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT profile path is not a directory",
        ));
    }
    Ok(canonical)
}

pub(in crate::local_pool::profiles::codex) fn parse_account_backup_snapshot(
    snapshot: &Option<Vec<u8>>,
    path: &Path,
) -> Result<Option<AccountProfileBackup>> {
    let Some(content) = snapshot_text(snapshot, path)? else {
        return Ok(None);
    };
    parse_account_backup(content, path).map(Some)
}

pub(in crate::local_pool::profiles::codex) fn parse_account_backup(
    content: &str,
    path: &Path,
) -> Result<AccountProfileBackup> {
    let backup: AccountProfileBackup = serde_json::from_str(content).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!(
                "ChatGPT account profile backup is invalid at {}: {error}",
                path.display()
            ),
        )
    })?;
    if backup.version != 1
        || backup.profile_dir.trim().is_empty()
        || backup.managed_account_id.trim().is_empty()
        || backup.managed_access_hash.len() != 64
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT account profile backup has invalid metadata",
        ));
    }
    Ok(backup)
}

pub(in crate::local_pool::profiles::codex) fn serialize_account_backup(
    backup: &AccountProfileBackup,
) -> Result<String> {
    let content = serde_json::to_string_pretty(backup).map_err(LocalPoolError::invalid_state)?;
    Ok(format!("{content}\n"))
}

pub(in crate::local_pool::profiles::codex) fn binding_from_backup(
    backup: &AccountProfileBackup,
    active: bool,
) -> ProfileBinding {
    ProfileBinding {
        profile_dir: backup.profile_dir.clone(),
        credential_kind: ProfileCredentialKind::OAuthAccount,
        credential_id: backup.managed_account_id.clone(),
        bound_oauth_account_id: None,
        active,
    }
}

pub(in crate::local_pool::profiles::codex) fn rollback_account_backup(
    created: bool,
    backup_path: &Path,
    attempted_content: &str,
    previous_snapshot: &Option<Vec<u8>>,
    backup: &AccountProfileBackup,
    secrets: &impl SecretBackend,
) -> Result<()> {
    rollback_file(backup_path, attempted_content, previous_snapshot)?;
    cleanup_created_account_backup_secret(created, backup, secrets)
}

pub(in crate::local_pool::profiles::codex) fn cleanup_created_account_backup_secret(
    created: bool,
    backup: &AccountProfileBackup,
    secrets: &impl SecretBackend,
) -> Result<()> {
    if !created {
        return Ok(());
    }
    super::delete_backup_secrets(
        backup.previous_auth_secret_ref.as_deref(),
        backup.projection_secret_ref.as_deref(),
        secrets,
    )
}

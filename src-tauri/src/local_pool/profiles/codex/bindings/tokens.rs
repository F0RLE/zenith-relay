use super::super::*;

pub(crate) fn managed_account_token_update(
    codex_home: &Path,
    backup_root: &Path,
    account_id: &str,
    current_tokens: &TokenSet,
    provider_account_id: &str,
) -> Result<Option<ManagedAccountTokenUpdate>> {
    let _profile_guard = lock_codex_profile();
    let mut update = None;

    if backup_root.exists() {
        for entry in fs::read_dir(backup_root).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with(ACCOUNT_BACKUP_PREFIX) || !name.ends_with(".json") {
                continue;
            }
            let backup_path = entry.path();
            let content = fs::read_to_string(&backup_path)
                .map_err(|error| io_error_at(&backup_path, error))?;
            let backup = parse_account_backup(&content, &backup_path)?;
            if backup.managed_account_id != account_id {
                continue;
            }
            merge_managed_token_update(
                &mut update,
                read_managed_account_token_update(
                    Path::new(&backup.profile_dir),
                    current_tokens,
                    provider_account_id,
                )?,
            )?;
        }
    }

    if let Some(backup) = local_backup(codex_home, backup_root)? {
        if backup.bound_oauth_account_id.as_deref() == Some(account_id) {
            merge_managed_token_update(
                &mut update,
                read_managed_account_token_update(codex_home, current_tokens, provider_account_id)?,
            )?;
        }
    }

    Ok(update)
}

fn read_managed_account_token_update(
    profile_dir: &Path,
    current_tokens: &TokenSet,
    provider_account_id: &str,
) -> Result<Option<ManagedAccountTokenUpdate>> {
    if !profile_dir.exists() {
        return Ok(None);
    }
    let profile_dir = canonical_profile_dir(profile_dir)?;
    let auth_path = profile_dir.join(AUTH_FILE);
    let snapshot = read_optional_bytes(&auth_path)?;
    let Some(content) = snapshot_text(&snapshot, &auth_path)? else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return Ok(None);
    };
    if auth_credential_kind(&value) != Some(ProfileCredentialKind::OAuthAccount) {
        return Ok(None);
    }
    let Some(tokens) = value.get("tokens").and_then(serde_json::Value::as_object) else {
        return Ok(None);
    };
    if tokens
        .get("account_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        != Some(provider_account_id)
    {
        return Ok(None);
    }
    let Some(access_token) = managed_token(tokens, "access_token") else {
        return Ok(None);
    };
    let Some(refresh_token) = managed_token(tokens, "refresh_token") else {
        return Ok(None);
    };
    let id_token = managed_token(tokens, "id_token").map(str::to_string);
    let update = ManagedAccountTokenUpdate {
        access_token: access_token.to_string(),
        refresh_token: refresh_token.to_string(),
        id_token,
    };
    // A desktop client can rotate a refresh or ID token without changing the
    // access token. Treat every supplied credential component as a generation
    // change, while preserving a stored ID token when the profile simply omits
    // that optional field.
    let id_token_changed = update
        .id_token
        .as_deref()
        .is_some_and(|value| Some(value) != current_tokens.id_token());
    if update.access_token == current_tokens.access_token()
        && update.refresh_token == current_tokens.refresh_token().unwrap_or_default()
        && !id_token_changed
    {
        return Ok(None);
    }
    Ok(Some(update))
}

pub(in crate::local_pool::profiles::codex) fn managed_token<'a>(
    tokens: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    tokens
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= MAX_MANAGED_TOKEN_BYTES
                && !value.bytes().any(|byte| byte.is_ascii_control())
        })
}

fn merge_managed_token_update(
    current: &mut Option<ManagedAccountTokenUpdate>,
    next: Option<ManagedAccountTokenUpdate>,
) -> Result<()> {
    let Some(next) = next else {
        return Ok(());
    };
    if current.as_ref().is_some_and(|current| {
        current.access_token != next.access_token
            || current.refresh_token != next.refresh_token
            || current.id_token != next.id_token
    }) {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "managed ChatGPT profiles contain conflicting token generations",
        ));
    }
    *current = Some(next);
    Ok(())
}

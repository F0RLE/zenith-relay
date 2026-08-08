use super::*;

pub(super) fn parse_config(content: &str) -> Result<DocumentMut> {
    content.parse::<DocumentMut>().map_err(|error| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("ChatGPT config is not valid TOML: {error}"),
        )
    })
}

pub(super) fn validate_config_shape(document: &DocumentMut) -> Result<()> {
    if document.get("model_provider").is_some()
        && document
            .get("model_provider")
            .and_then(Item::as_str)
            .is_none()
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model_provider must be a string",
        ));
    }
    if document.get("model_providers").is_some()
        && document
            .get("model_providers")
            .and_then(Item::as_table)
            .is_none()
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model_providers must be a table",
        ));
    }
    if document.get("model_reasoning_effort").is_some()
        && document
            .get("model_reasoning_effort")
            .and_then(Item::as_str)
            .is_none()
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model_reasoning_effort must be a string",
        ));
    }
    Ok(())
}

pub(super) fn attach_config(
    document: &mut DocumentMut,
    base_url: &str,
    local_key: &str,
    model_catalog_path: Option<&str>,
    previous_model_catalog: Option<&str>,
) {
    // A global Codex effort overrides each model catalog entry. While Relay is
    // active it must be absent so automatic models use Relay's `medium`
    // default and a manual per-model rule remains authoritative.
    document.remove("model_reasoning_effort");
    document["model_provider"] = value(PROVIDER_ID);
    restore_root_string(
        document,
        "model_catalog_json",
        model_catalog_path.or(previous_model_catalog),
    );
    if document
        .get("model_providers")
        .and_then(Item::as_table)
        .is_none()
    {
        document["model_providers"] = Item::Table(Table::new());
    }
    document["model_providers"][PROVIDER_ID] = Item::Table(Table::new());
    let provider = &mut document["model_providers"][PROVIDER_ID];
    provider["name"] = value("Zenith Relay Local");
    provider["base_url"] = value(base_url);
    provider["wire_api"] = value("responses");
    provider["requires_openai_auth"] = value(true);
    provider["experimental_bearer_token"] = value(local_key);
    provider["supports_websockets"] = value(false);
}

pub(super) fn restore_config(
    document: &mut DocumentMut,
    previous_model_provider: Option<&str>,
    previous_model_catalog: Option<&str>,
) {
    remove_managed_provider(document);
    restore_root_string(document, "model_provider", previous_model_provider);
    restore_root_string(document, "model_catalog_json", previous_model_catalog);
}

pub(super) fn restore_local_config(
    document: &mut DocumentMut,
    backup: &ProfileBackup,
    previous_model_catalog: Option<&str>,
) {
    restore_config(
        document,
        backup.previous_model_provider.as_deref(),
        previous_model_catalog,
    );
    if backup.managed_model_reasoning_effort_cleared {
        restore_root_string(
            document,
            "model_reasoning_effort",
            backup.previous_model_reasoning_effort.as_deref(),
        );
    }
}

pub(super) const MANAGED_SNAPSHOT_AUTH_KEYS: &[&str] =
    &["OPENAI_API_KEY", "auth_mode", "last_refresh", "tokens"];

pub(super) fn managed_snapshot_scope(
    profile_dir: &Path,
    backup_root: &Path,
    document: &DocumentMut,
    auth: &Option<Vec<u8>>,
    auth_path: &Path,
) -> Result<ManagedSnapshotScope> {
    if let Some(path) = account_backup_for_profile(profile_dir, backup_root)? {
        let bytes = read_optional_bytes(&path)?;
        let backup = parse_account_backup_snapshot(&bytes, &path)?.ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "ChatGPT account profile backup disappeared during snapshot restore",
            )
        })?;
        if backup.profile_dir != profile_dir.to_string_lossy()
            || !account_managed_config_matches(document)
            || !account_auth_matches_snapshot(auth, auth_path, &backup.managed_access_hash)?
        {
            return Err(profile_restore_blocked());
        }
        return Ok(ManagedSnapshotScope::OAuthAccount);
    }

    let Some(backup) = local_backup(profile_dir, backup_root)? else {
        return Ok(ManagedSnapshotScope::NoBinding);
    };
    if !managed_config_matches(document, &backup)
        || !managed_auth_matches_snapshot(auth, auth_path, &backup)?
    {
        return Err(profile_restore_blocked());
    }
    Ok(ManagedSnapshotScope::LocalGateway)
}

pub(super) fn merge_managed_snapshot_config(
    current: Option<&str>,
    snapshot: Option<&str>,
    scope: ManagedSnapshotScope,
) -> Result<Option<String>> {
    let mut document = parse_config(current.unwrap_or_default())?;
    validate_config_shape(&document)?;
    let snapshot = snapshot.map(parse_config).transpose()?;
    if let Some(snapshot) = snapshot.as_ref() {
        validate_config_shape(snapshot)?;
    }
    let before = document.to_string();

    let model_provider = snapshot.as_ref().and_then(root_model_provider);
    if model_provider.as_deref() == Some(PROVIDER_ID) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "a Relay-managed snapshot cannot restore another Relay binding",
        ));
    }
    restore_root_string(&mut document, "model_provider", model_provider.as_deref());
    let snapshot_value = |key| {
        snapshot
            .as_ref()
            .map(|snapshot| optional_root_string(snapshot, key))
            .transpose()
            .map(Option::flatten)
    };
    match scope {
        ManagedSnapshotScope::LocalGateway => {
            let catalog = snapshot_value("model_catalog_json")?;
            restore_root_string(&mut document, "model_catalog_json", catalog.as_deref());
        }
        ManagedSnapshotScope::OAuthAccount => {
            let base_url = snapshot_value("openai_base_url")?;
            restore_root_string(&mut document, "openai_base_url", base_url.as_deref());
        }
        ManagedSnapshotScope::NoBinding => {}
    }
    // Snapshot recovery always detaches Relay. Re-creating its provider here
    // would leave Codex pointed at a credential whose backup was discarded.
    remove_managed_provider(&mut document);

    let restored = document.to_string();
    if restored == before {
        return Ok(current.map(ToOwned::to_owned));
    }
    if restored.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(restored))
}

pub(super) fn optional_root_string(document: &DocumentMut, key: &str) -> Result<Option<String>> {
    let Some(item) = document.get(key) else {
        return Ok(None);
    };
    item.as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                format!("ChatGPT {key} must be a string"),
            )
        })
        .map(Some)
}

pub(super) fn merge_managed_snapshot_auth(
    current: Option<&str>,
    snapshot: Option<&str>,
) -> Result<Option<String>> {
    let snapshot = snapshot
        .map(parse_profile_auth)
        .transpose()?
        .unwrap_or_default();
    let mut current_value = match current {
        Some(content) => parse_profile_auth(content)?,
        None => serde_json::Map::new(),
    };
    let before = current_value.clone();
    for key in MANAGED_SNAPSHOT_AUTH_KEYS {
        match snapshot.get(*key) {
            Some(value) => {
                current_value.insert((*key).to_string(), value.clone());
            }
            None => {
                current_value.remove(*key);
            }
        }
    }
    if current_value == before {
        return Ok(current.map(ToOwned::to_owned));
    }
    if current_value.is_empty() {
        return Ok(None);
    }
    let content = serde_json::to_string_pretty(&serde_json::Value::Object(current_value))
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?;
    Ok(Some(format!("{content}\n")))
}

pub(super) fn parse_profile_auth(
    content: &str,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    let value = serde_json::from_str::<serde_json::Value>(content).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("ChatGPT auth is not valid JSON: {error}"),
        )
    })?;
    value.as_object().cloned().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT auth must contain a JSON object",
        )
    })
}

pub(super) fn restore_root_string(document: &mut DocumentMut, key: &str, previous: Option<&str>) {
    match previous {
        Some(previous) => document[key] = value(previous),
        None => {
            document.remove(key);
        }
    }
}

pub(super) fn remove_managed_provider(document: &mut DocumentMut) {
    if let Some(model_providers) = document["model_providers"].as_table_mut() {
        model_providers.remove(PROVIDER_ID);
        if model_providers.is_empty() {
            document.remove("model_providers");
        }
    }
}

pub(super) fn root_model_provider(document: &DocumentMut) -> Option<String> {
    document
        .get("model_provider")
        .and_then(Item::as_str)
        .map(ToOwned::to_owned)
}

pub(super) fn root_model_catalog_json(document: &DocumentMut) -> Option<String> {
    document
        .get("model_catalog_json")
        .and_then(Item::as_str)
        .map(ToOwned::to_owned)
}

pub(super) fn root_model_reasoning_effort(document: &DocumentMut) -> Option<String> {
    document
        .get("model_reasoning_effort")
        .and_then(Item::as_str)
        .map(ToOwned::to_owned)
}

pub(super) fn external_model_catalog(
    document: &DocumentMut,
    backup: &ProfileBackup,
) -> Option<String> {
    let current = root_model_catalog_json(document);
    if current.as_deref() == backup.managed_model_catalog_path.as_deref() {
        backup.previous_model_catalog_json.clone()
    } else {
        current
    }
}

pub(super) fn model_catalog_to_restore(
    document: &DocumentMut,
    backup: &ProfileBackup,
) -> Option<String> {
    if backup.managed_model_catalog_path.is_some() {
        backup.previous_model_catalog_json.clone()
    } else {
        root_model_catalog_json(document)
    }
}

pub(super) fn root_openai_base_url(document: &DocumentMut) -> Option<String> {
    document
        .get("openai_base_url")
        .and_then(Item::as_str)
        .map(ToOwned::to_owned)
}

pub(super) fn document_has_provider(document: &DocumentMut) -> bool {
    document
        .get("model_providers")
        .and_then(Item::as_table)
        .is_some_and(|providers| providers.contains_key(PROVIDER_ID))
}

pub(super) fn managed_config_matches(document: &DocumentMut, backup: &ProfileBackup) -> bool {
    root_model_provider(document).as_deref() == Some(PROVIDER_ID)
        && (backup.managed_model_catalog_path.is_none()
            || root_model_catalog_json(document).as_deref()
                == backup.managed_model_catalog_path.as_deref())
        && managed_provider_matches(document, backup)
        && (!backup.managed_model_reasoning_effort_cleared
            || document.get("model_reasoning_effort").is_none())
}

pub(super) fn previous_config_matches(document: &DocumentMut, backup: &ProfileBackup) -> bool {
    root_model_provider(document) == backup.previous_model_provider
        && root_model_catalog_json(document) == backup.previous_model_catalog_json
        && (!backup.managed_model_reasoning_effort_cleared
            || root_model_reasoning_effort(document) == backup.previous_model_reasoning_effort)
}

pub(super) fn external_provider_took_over(document: &DocumentMut, backup: &ProfileBackup) -> bool {
    root_model_provider(document).is_some_and(|provider| provider != PROVIDER_ID)
        && managed_provider_matches(document, backup)
}

pub(super) fn external_account_provider_took_over(codex_home: &Path) -> Result<bool> {
    let config_path = canonical_profile_dir(codex_home)?.join(CONFIG_FILE);
    let config = read_optional_bytes(&config_path)?;
    let document = parse_config(snapshot_text(&config, &config_path)?.unwrap_or_default())?;
    Ok(root_model_provider(&document)
        .is_some_and(|provider| provider != "openai" && provider != PROVIDER_ID))
}

pub(super) fn managed_provider_matches(document: &DocumentMut, backup: &ProfileBackup) -> bool {
    document
        .get("model_providers")
        .and_then(Item::as_table)
        .and_then(|providers| providers.get(PROVIDER_ID))
        .and_then(Item::as_table)
        .is_some_and(|provider| {
            provider
                .get("name")
                .and_then(Item::as_str)
                .is_some_and(|name| name == "Zenith Relay Local")
                && provider
                    .get("base_url")
                    .and_then(Item::as_str)
                    .is_some_and(|base_url| {
                        base_url.trim_end_matches('/') == backup.managed_base_url
                    })
                && provider
                    .get("wire_api")
                    .and_then(Item::as_str)
                    .is_some_and(|wire_api| wire_api == "responses")
                && provider.get("requires_openai_auth").and_then(Item::as_bool) == Some(true)
                && (!backup.managed_bearer_in_config
                    || provider
                        .get("experimental_bearer_token")
                        .and_then(Item::as_str)
                        .is_some_and(|token| key_hash(token.trim()) == backup.managed_key_hash))
                && provider.get("supports_websockets").and_then(Item::as_bool)
                    == Some(backup.managed_supports_websockets)
        })
}

pub(super) fn auth_content(local_key: &str) -> String {
    format!(
        "{{\n  \"OPENAI_API_KEY\": \"{}\",\n  \"auth_mode\": \"apikey\"\n}}\n",
        escape_json_string(local_key)
    )
}

pub(super) fn auth_matches_snapshot(
    snapshot: &Option<Vec<u8>>,
    path: &Path,
    expected_hash: &str,
) -> Result<bool> {
    let Some(content) = snapshot_text(snapshot, path)? else {
        return Ok(false);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(content) else {
        return Ok(false);
    };
    Ok(value
        .get("OPENAI_API_KEY")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|key| key_hash(key.trim()) == expected_hash)
        && value
            .get("auth_mode")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|mode| mode == "apikey"))
}

pub(super) fn managed_auth_matches_snapshot(
    snapshot: &Option<Vec<u8>>,
    path: &Path,
    backup: &ProfileBackup,
) -> Result<bool> {
    match (
        backup.bound_oauth_account_id.as_deref(),
        backup.managed_oauth_access_hash.as_deref(),
    ) {
        (Some(account_id), Some(access_hash))
            if !account_id.trim().is_empty() && access_hash.len() == 64 =>
        {
            account_auth_matches_snapshot(snapshot, path, access_hash)
        }
        (Some(account_id), None) if !account_id.trim().is_empty() => {
            auth_matches_snapshot(snapshot, path, &backup.managed_key_hash)
        }
        (None, None) => auth_matches_snapshot(snapshot, path, &backup.managed_key_hash),
        _ => Ok(false),
    }
}

pub(super) fn previous_auth_matches_snapshot(
    snapshot: &Option<Vec<u8>>,
    backup: &ProfileBackup,
) -> bool {
    match backup.previous_auth_hash.as_deref() {
        Some(expected_hash) => snapshot
            .as_deref()
            .is_some_and(|content| bytes_hash(content) == expected_hash),
        None if backup.previous_auth_secret_ref.is_none() => snapshot.is_none(),
        None => false,
    }
}

pub(super) fn key_hash(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

pub(super) fn bytes_hash(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

//! Explicit recovery restores connection state, not unrelated preferences or
//! the conversation files. Attachment rollback retains its separate fences.
use super::*;

const CONNECTION_FIELDS: &[&str] = &[
    "model_provider",
    "model_catalog_json",
    "chatgpt_base_url",
    "openai_base_url",
    "forced_login_method",
    "forced_chatgpt_workspace_id",
    "cli_auth_credentials_store",
];

/// History is scoped by provider, not by credential type: an OAuth credential
/// can also be used through the local gateway. Use the saved active route.
pub(in crate::local_pool::profiles) fn history_provider(
    snapshot: &UserProfileSnapshot,
) -> Result<String> {
    let config = parse_config(snapshot.config.as_deref().unwrap_or_default())?;
    validate_config_shape(&config)?;
    let active_profile = config.get("profile").and_then(Item::as_str);
    let provider = active_profile
        .and_then(|name| config.get("profiles")?.get(name)?.get("model_provider"))
        .or_else(|| config.get("model_provider"));
    match provider {
        Some(provider) => provider
            .as_str()
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
            .ok_or_else(|| {
                LocalPoolError::new(ErrorCode::RecoveryRequired, "Snapshot provider is invalid")
            }),
        None => Ok(NATIVE_PROVIDER_ID.to_owned()),
    }
}

pub(super) fn restore_connection(
    saved: &UserProfileSnapshot,
    current_config: Option<&str>,
    current_auth: Option<&str>,
) -> Result<UserProfileSnapshot> {
    let saved_config = parse_config(saved.config.as_deref().unwrap_or_default())?;
    validate_config_shape(&saved_config)?;
    let mut config = parse_config(current_config.unwrap_or_default())?;
    validate_config_shape(&config)?;
    let provider = history_provider(saved)?;
    let current_provider = history_provider(&UserProfileSnapshot {
        config: current_config.map(str::to_owned),
        auth: None,
    })?;
    let route_changed = current_config.is_none()
        || provider != current_provider
        || provider_config(&saved_config, &provider) != provider_config(&config, &provider)
        || ["chatgpt_base_url", "openai_base_url"].iter().any(|key| {
            saved_config.get(key).and_then(Item::as_str) != config.get(key).and_then(Item::as_str)
        });
    let active_profile = saved_config.get("profile").and_then(Item::as_str);
    // Root defaults participate in the selected connection. Only the selected
    // named profile is restored; inactive profiles retain their own overrides.
    restore_fields(saved_config.as_table(), config.as_table_mut());
    restore_route_models(
        saved_config.as_table(),
        config.as_table_mut(),
        route_changed,
    );
    copy_field(saved_config.as_table(), config.as_table_mut(), "profile");
    restore_auth_backend(&saved_config, &mut config)?;

    if let Some(name) = active_profile {
        let saved_profile = saved_config
            .get("profiles")
            .and_then(|profiles| profiles.get(name))
            .and_then(Item::as_table_like)
            .ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "Snapshot selected profile is missing",
                )
            })?;
        if config.get("profiles").is_none() {
            config["profiles"] = Item::Table(Table::new());
        }
        let profiles = config
            .get_mut("profiles")
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "Profile settings must be a table",
                )
            })?;
        if profiles.get(name).is_none() {
            profiles.insert(name, Item::Table(Table::new()));
        }
        let profile = profiles
            .get_mut(name)
            .and_then(Item::as_table_like_mut)
            .ok_or_else(|| {
                LocalPoolError::new(ErrorCode::RecoveryRequired, "Named profile must be a table")
            })?;
        restore_fields(saved_profile, profile);
        restore_route_models(saved_profile, profile, route_changed);
    }
    {
        let saved_provider = saved_config
            .get("model_providers")
            .and_then(|table| table.get(&provider));
        if let Some(saved_provider) = saved_provider {
            if config.get("model_providers").is_none() {
                config["model_providers"] = Item::Table(Table::new());
            }
            config["model_providers"][&provider] = saved_provider.clone();
        } else if let Some(table) = config
            .get_mut("model_providers")
            .and_then(Item::as_table_like_mut)
        {
            table.remove(&provider);
        }
    }
    let config = config.to_string();
    Ok(UserProfileSnapshot {
        config: if config.trim().is_empty() && saved.config.is_none() {
            None
        } else {
            Some(config)
        },
        auth: projection::merge_auth(current_auth, saved.auth.as_deref())?,
    })
}

fn provider_config(config: &DocumentMut, provider: &str) -> Option<String> {
    config
        .get("model_providers")
        .and_then(|table| table.get(provider))
        .map(|definition| {
            ["base_url", "wire_api"]
                .iter()
                .map(|key| {
                    definition
                        .get(key)
                        .and_then(Item::as_str)
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
}

fn restore_auth_backend(saved: &DocumentMut, current: &mut DocumentMut) -> Result<()> {
    let saved = saved
        .get("features")
        .and_then(|features| features.get("secret_auth_storage"));
    if saved.is_some() && current.get("features").is_none() {
        current["features"] = Item::Table(Table::new());
    }
    if let Some(features) = current.get_mut("features") {
        let features = features
            .as_table_like_mut()
            .ok_or_else(|| LocalPoolError::invalid_state("Codex features must be a table"))?;
        match saved {
            Some(saved) => {
                features.insert("secret_auth_storage", saved.clone());
            }
            None => {
                features.remove("secret_auth_storage");
            }
        }
    }
    Ok(())
}

/// Connection ownership is independent of model preferences and inactive
/// profiles. Automatic detach still uses the original projection fences.
pub(super) fn account_connection_matches(config: &DocumentMut) -> bool {
    let selected = config
        .get("profile")
        .and_then(Item::as_str)
        .and_then(|name| config.get("profiles")?.get(name)?.as_table_like());
    let field = |key| {
        selected
            .and_then(|profile| profile.get(key))
            .or_else(|| config.get(key))
    };
    field("model_provider")
        .and_then(Item::as_str)
        .is_none_or(|provider| provider == NATIVE_PROVIDER_ID)
        && ["model_catalog_json", "chatgpt_base_url", "openai_base_url"]
            .iter()
            .all(|key| field(key).is_none())
        && config
            .get("model_providers")
            .and_then(|providers| providers.get(NATIVE_PROVIDER_ID))
            .is_none()
}

fn restore_route_models(
    saved: &dyn toml_edit::TableLike,
    current: &mut dyn toml_edit::TableLike,
    route_changed: bool,
) {
    if route_changed {
        for key in ["model", "review_model"] {
            copy_field(saved, current, key);
        }
    }
}

fn restore_fields(saved: &dyn toml_edit::TableLike, current: &mut dyn toml_edit::TableLike) {
    for key in CONNECTION_FIELDS {
        copy_field(saved, current, key);
    }
}

fn copy_field(saved: &dyn toml_edit::TableLike, current: &mut dyn toml_edit::TableLike, key: &str) {
    if let Some(value) = saved.get(key) {
        current.insert(key, value.clone());
    } else {
        current.remove(key);
    }
}

pub(super) fn references_catalog(config: Option<&str>, path: &Path) -> Result<bool> {
    let config = parse_config(config.unwrap_or_default())?;
    let path = portable_path_string(path);
    let matches = |table: &dyn toml_edit::TableLike| {
        table
            .get("model_catalog_json")
            .and_then(Item::as_str)
            .is_some_and(|value| portable_path_value(value) == path)
    };
    Ok(matches(config.as_table())
        || config
            .get("profiles")
            .and_then(Item::as_table_like)
            .is_some_and(|profiles| {
                profiles
                    .iter()
                    .filter_map(|(_, profile)| profile.as_table_like())
                    .any(matches)
            }))
}

pub(in crate::local_pool::profiles) fn capture_managed_catalog(
    backup_root: &Path,
    saved: &UserProfileSnapshot,
) -> Result<Option<String>> {
    fs::create_dir_all(backup_root).map_err(io_error)?;
    let path = managed_model_catalog_path(backup_root)?;
    let config = parse_config(saved.config.as_deref().unwrap_or_default())?;
    let selected = config
        .get("profile")
        .and_then(Item::as_str)
        .and_then(|name| config.get("profiles")?.get(name)?.get("model_catalog_json"))
        .or_else(|| config.get("model_catalog_json"));
    if !selected
        .and_then(Item::as_str)
        .is_some_and(|selected| portable_path_value(selected) == portable_path_string(&path))
    {
        return Ok(None);
    }
    let bytes =
        crate::files::read_regular_file_bounded(&path, catalog::MAX_MODEL_CATALOG_BYTES as u64)
            .map_err(|_| {
                LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "Snapshot model catalog is unavailable",
                )
            })?;
    let content = String::from_utf8(bytes).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Snapshot model catalog is invalid",
        )
    })?;
    validate_snapshot_catalog(&content)?;
    Ok(Some(content))
}

pub(in crate::local_pool::profiles) fn validate_snapshot_catalog(content: &str) -> Result<()> {
    if content.len() > catalog::MAX_MODEL_CATALOG_BYTES
        || !serde_json::from_str::<Value>(content)
            .ok()
            .is_some_and(|catalog| catalog.get("models").is_some_and(Value::is_array))
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Snapshot model catalog is invalid or too large",
        ));
    }
    Ok(())
}

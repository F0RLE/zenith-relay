use super::*;

pub fn sync_default_service_tier(
    codex_home: &Path,
    default_service_tier: DefaultServiceTier,
) -> Result<()> {
    let _profile_guard = lock_codex_profile();
    fs::create_dir_all(codex_home).map_err(io_error)?;
    let config_path = codex_home.join(CONFIG_FILE);
    let state_path = codex_home.join(GLOBAL_STATE_FILE);
    let original_config = read_optional_bytes(&config_path)?;
    let original_state = read_optional_bytes(&state_path)?;

    let mut document =
        parse_config(snapshot_text(&original_config, &config_path)?.unwrap_or_default())?;
    let (desktop_service_tier, top_level_service_tier) = match default_service_tier {
        DefaultServiceTier::Standard => {
            if let Some(desktop) = document.get_mut("desktop") {
                desktop
                    .as_table_mut()
                    .ok_or_else(|| {
                        LocalPoolError::new(
                            ErrorCode::InvalidState,
                            "Codex desktop settings must be a table",
                        )
                    })?
                    .remove(DESKTOP_DEFAULT_SERVICE_TIER_KEY);
            }
            (None, "default")
        }
        DefaultServiceTier::Fast => {
            if document.get("desktop").is_none() {
                document["desktop"] = Item::Table(Table::new());
            }
            let desktop = document["desktop"].as_table_mut().ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "Codex desktop settings must be a table",
                )
            })?;
            desktop[DESKTOP_DEFAULT_SERVICE_TIER_KEY] = value("priority");
            (Some("priority"), "priority")
        }
        DefaultServiceTier::Ultrafast => {
            if document.get("desktop").is_none() {
                document["desktop"] = Item::Table(Table::new());
            }
            let desktop = document["desktop"].as_table_mut().ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "Codex desktop settings must be a table",
                )
            })?;
            desktop[DESKTOP_DEFAULT_SERVICE_TIER_KEY] = value("ultrafast");
            (Some("ultrafast"), "ultrafast")
        }
    };
    document[TOP_LEVEL_SERVICE_TIER_KEY] = value(top_level_service_tier);
    let next_config = document.to_string();

    let mut global_state_document = match snapshot_text(&original_state, &state_path)? {
        Some(content) => serde_json::from_str::<Value>(content).map_err(|error| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!("Codex global state is not valid JSON: {error}"),
            )
        })?,
        None => Value::Object(Default::default()),
    };
    let global_state = global_state_document.as_object_mut().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Codex global state must be a JSON object",
        )
    })?;
    let persisted_atom_state = global_state
        .entry(PERSISTED_ATOM_STATE_KEY.to_string())
        .or_insert_with(|| Value::Object(Default::default()));
    if !persisted_atom_state.is_object() {
        *persisted_atom_state = Value::Object(Default::default());
    }
    let persisted_atom_state = persisted_atom_state
        .as_object_mut()
        .expect("persisted atom state was normalized to an object");
    persisted_atom_state.insert(
        DESKTOP_DEFAULT_SERVICE_TIER_KEY.to_string(),
        desktop_service_tier.map_or(Value::Null, |tier| Value::String(tier.to_string())),
    );
    persisted_atom_state.insert(SERVICE_TIER_CHANGED_KEY.to_string(), Value::Bool(true));
    let next_state = serde_json::to_string(global_state).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("Codex global state could not be serialized: {error}"),
        )
    })?;

    let config_changed = original_config
        .as_deref()
        .map_or(!next_config.is_empty(), |existing_config| {
            existing_config != next_config.as_bytes()
        });
    if config_changed {
        replace_if_unchanged(&config_path, &original_config, &next_config)?;
    }
    if original_state.as_deref() != Some(next_state.as_bytes()) {
        if let Err(error) = replace_if_unchanged(&state_path, &original_state, &next_state) {
            return Err(if config_changed {
                with_rollback(
                    error,
                    rollback_file(&config_path, &next_config, &original_config),
                )
            } else {
                error
            });
        }
    }
    Ok(())
}

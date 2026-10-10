use super::super::*;
use super::load;

pub(in crate::local_pool::profiles::codex) fn restore(
    secret_ref: &str,
    config: Option<&str>,
    auth: Option<&str>,
    secrets: &impl SecretBackend,
) -> Result<UserProfileSnapshot> {
    let saved = load(secret_ref, secrets)?;
    Ok(UserProfileSnapshot {
        config: restore_config_text(saved.config_before.as_deref(), &saved.config_after, config)?,
        auth: super::auth::merge_auth(auth, saved.auth_before.as_deref())?,
    })
}

/// Both account and local profile restore use the same encrypted projection
/// when present; older backups merge only the managed auth and config leaves.
pub(in crate::local_pool::profiles::codex) fn restore_from_backup(
    secret_ref: Option<&str>,
    document: &DocumentMut,
    config: (&Path, &Option<Vec<u8>>),
    auth: (&Path, &Option<Vec<u8>>),
    auth_matches_managed: bool,
    previous_auth: Option<&str>,
    secrets: &impl SecretBackend,
) -> Result<UserProfileSnapshot> {
    match secret_ref {
        Some(secret_ref) => restore(
            secret_ref,
            snapshot_text(config.1, config.0)?,
            if auth_matches_managed {
                snapshot_text(auth.1, auth.0)?
            } else {
                None
            },
            secrets,
        ),
        None => Ok(UserProfileSnapshot {
            config: Some(document.to_string()),
            auth: if auth_matches_managed {
                super::auth::merge_auth(snapshot_text(auth.1, auth.0)?, previous_auth)?
            } else {
                None
            },
        }),
    }
}

fn restore_config_text(
    before: Option<&str>,
    after: &str,
    current_text: Option<&str>,
) -> Result<Option<String>> {
    if current_text == Some(after) || current_text == before {
        return Ok(before.map(str::to_owned));
    }
    let before_doc = parse_config(before.unwrap_or_default())?;
    let after_doc = parse_config(after)?;
    let mut current_doc = parse_config(current_text.unwrap_or_default())?;
    restore_table(
        before_doc.as_table(),
        after_doc.as_table(),
        current_doc.as_table_mut(),
    );
    let restored = current_doc.to_string();
    if restored.trim().is_empty() && before.is_none() {
        Ok(None)
    } else {
        Ok(Some(restored))
    }
}

fn same(left_item: Option<&Item>, right_item: Option<&Item>) -> bool {
    // Formatting is not an ownership change. Values have a canonical Display
    // after clearing surrounding decoration; tables are compared recursively.
    match (left_item, right_item) {
        (None, None) => true,
        (Some(left_item), Some(right_item)) => {
            match (left_item.as_table_like(), right_item.as_table_like()) {
                (Some(left_table), Some(right_table)) => {
                    left_table.len() == right_table.len()
                        && left_table
                            .iter()
                            .all(|(key, nested_item)| same(Some(nested_item), right_table.get(key)))
                }
                _ => match (left_item.as_str(), right_item.as_str()) {
                    (Some(left_text), Some(right_text)) => left_text == right_text,
                    _ => normalized(left_item) == normalized(right_item),
                },
            }
        }
        _ => false,
    }
}

fn normalized(toml_item: &Item) -> String {
    let mut normalized_item = toml_item.clone();
    if let Some(value_node) = normalized_item.as_value_mut() {
        value_node.decor_mut().clear();
    }
    normalized_item.to_string()
}

fn restore_table(
    saved_before: &dyn toml_edit::TableLike,
    managed_after: &dyn toml_edit::TableLike,
    current_config: &mut dyn toml_edit::TableLike,
) {
    let keys: std::collections::BTreeSet<_> = saved_before
        .iter()
        .chain(managed_after.iter())
        .map(|(key, _)| key.to_owned())
        .collect();
    for key in keys {
        let previous_value = saved_before.get(&key);
        let managed_value = managed_after.get(&key);
        if same(previous_value, managed_value) || same(current_config.get(&key), previous_value) {
            continue;
        }
        if let (Some(managed_table), Some(existing_table)) = (
            managed_value.and_then(Item::as_table_like),
            current_config
                .get_mut(&key)
                .and_then(Item::as_table_like_mut),
        ) {
            let empty = Table::new();
            restore_table(
                previous_value
                    .and_then(Item::as_table_like)
                    .unwrap_or(&empty),
                managed_table,
                existing_table,
            );
            if previous_value.is_none() && existing_table.is_empty() {
                current_config.remove(&key);
            }
            continue;
        }
        if !same(current_config.get(&key), managed_value) {
            // An external edit owns this leaf. Undo the other Relay-owned
            // leaves without replacing the user's newer value.
            continue;
        }
        match previous_value {
            Some(previous_item) => {
                current_config.insert(&key, previous_item.clone());
            }
            None => {
                current_config.remove(&key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_preserves_nested_profiles_and_new_user_preferences() {
        let before =
            "# mine\nmodel_provider = 'custom'\n[profiles.work]\nmodel_provider = 'work'\n";
        let after = "model_provider = 'relay'\n[profiles.work]\nmodel_provider = 'work'\n[model_providers.relay]\nbase_url = 'local'\n";
        let user_config_text = format!("model = 'chosen'\n{after}");
        let restored_text = restore_config_text(Some(before), after, Some(&user_config_text))
            .unwrap()
            .unwrap();
        let restored_config = parse_config(&restored_text).unwrap();
        assert_eq!(restored_config["model"].as_str(), Some("chosen"));
        assert_eq!(restored_config["model_provider"].as_str(), Some("custom"));
        assert_eq!(
            restored_config["profiles"]["work"]["model_provider"].as_str(),
            Some("work")
        );
        assert!(restored_config.get("model_providers").is_none());
    }

    #[test]
    fn restore_reverts_ultra_picker_unless_the_user_changed_it() {
        let before = "model_provider = 'custom'\n";
        let after =
            "model_provider = 'relay'\n\n[desktop]\nshow-ultra-in-model-picker-slider = true\n";
        let restored = restore_config_text(Some(before), after, Some(after))
            .unwrap()
            .unwrap();
        let restored = parse_config(&restored).unwrap();
        assert_eq!(restored["model_provider"].as_str(), Some("custom"));
        assert!(restored.get("desktop").is_none());

        let user_off =
            "model_provider = 'relay'\n\n[desktop]\nshow-ultra-in-model-picker-slider = false\n";
        let kept = restore_config_text(Some(before), after, Some(user_off))
            .unwrap()
            .unwrap();
        let kept = parse_config(&kept).unwrap();
        assert_eq!(kept["model_provider"].as_str(), Some("custom"));
        assert_eq!(
            kept["desktop"]["show-ultra-in-model-picker-slider"].as_bool(),
            Some(false)
        );
    }

    #[test]
    fn exact_round_trip_preserves_bytes_and_absence() {
        let original = "# comment\r\nmodel_provider='mine'\r\n";
        assert_eq!(
            restore_config_text(
                Some(original),
                "model_provider='relay'",
                Some("model_provider='relay'")
            )
            .unwrap()
            .as_deref(),
            Some(original)
        );
        assert_eq!(
            restore_config_text(
                None,
                "model_provider='relay'",
                Some("model_provider='relay'")
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn changed_managed_endpoint_is_preserved() {
        let restored = restore_config_text(
            None,
            "[model_providers.relay]\nbase_url='local'",
            Some("[model_providers.relay]\nbase_url='external'"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            parse_config(&restored).unwrap()["model_providers"]["relay"]["base_url"].as_str(),
            Some("external")
        );
    }

    #[test]
    fn inline_provider_extension_survives_restore() {
        let restored_text = restore_config_text(
            None,
            "model_providers = {relay = {base_url = 'local'}}",
            Some("model_providers = {relay = {base_url = 'local', user_option = true}}"),
        )
        .unwrap()
        .unwrap();
        let restored_document = parse_config(&restored_text).unwrap();
        assert_eq!(
            restored_document["model_providers"]["relay"]["user_option"].as_bool(),
            Some(true)
        );
        assert!(restored_document["model_providers"]["relay"]
            .as_table_like()
            .unwrap()
            .get("base_url")
            .is_none());
    }
}

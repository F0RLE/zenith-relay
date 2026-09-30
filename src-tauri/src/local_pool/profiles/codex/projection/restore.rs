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
    current: Option<&str>,
) -> Result<Option<String>> {
    if current == Some(after) || current == before {
        return Ok(before.map(str::to_owned));
    }
    let before_doc = parse_config(before.unwrap_or_default())?;
    let after_doc = parse_config(after)?;
    let mut current_doc = parse_config(current.unwrap_or_default())?;
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

fn same(left: Option<&Item>, right: Option<&Item>) -> bool {
    // Formatting is not an ownership change. Values have a canonical Display
    // after clearing surrounding decoration; tables are compared recursively.
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => match (left.as_table_like(), right.as_table_like()) {
            (Some(left), Some(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .all(|(key, item)| same(Some(item), right.get(key)))
            }
            _ => match (left.as_str(), right.as_str()) {
                (Some(left), Some(right)) => left == right,
                _ => normalized(left) == normalized(right),
            },
        },
        _ => false,
    }
}

fn normalized(item: &Item) -> String {
    let mut item = item.clone();
    if let Some(value) = item.as_value_mut() {
        value.decor_mut().clear();
    }
    item.to_string()
}

fn restore_table(
    before: &dyn toml_edit::TableLike,
    after: &dyn toml_edit::TableLike,
    current: &mut dyn toml_edit::TableLike,
) {
    let keys: std::collections::BTreeSet<_> = before
        .iter()
        .chain(after.iter())
        .map(|(key, _)| key.to_owned())
        .collect();
    for key in keys {
        let previous = before.get(&key);
        let managed = after.get(&key);
        if same(previous, managed) || same(current.get(&key), previous) {
            continue;
        }
        if let (Some(managed), Some(existing)) = (
            managed.and_then(Item::as_table_like),
            current.get_mut(&key).and_then(Item::as_table_like_mut),
        ) {
            let empty = Table::new();
            restore_table(
                previous.and_then(Item::as_table_like).unwrap_or(&empty),
                managed,
                existing,
            );
            if previous.is_none() && existing.is_empty() {
                current.remove(&key);
            }
            continue;
        }
        if !same(current.get(&key), managed) {
            // An external edit owns this leaf. Undo the other Relay-owned
            // leaves without replacing the user's newer value.
            continue;
        }
        match previous {
            Some(item) => {
                current.insert(&key, item.clone());
            }
            None => {
                current.remove(&key);
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
        let current = format!("model = 'chosen'\n{after}");
        let result = restore_config_text(Some(before), after, Some(&current))
            .unwrap()
            .unwrap();
        let result = parse_config(&result).unwrap();
        assert_eq!(result["model"].as_str(), Some("chosen"));
        assert_eq!(result["model_provider"].as_str(), Some("custom"));
        assert_eq!(
            result["profiles"]["work"]["model_provider"].as_str(),
            Some("work")
        );
        assert!(result.get("model_providers").is_none());
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
        let result = restore_config_text(
            None,
            "model_providers = {relay = {base_url = 'local'}}",
            Some("model_providers = {relay = {base_url = 'local', user_option = true}}"),
        )
        .unwrap()
        .unwrap();
        let document = parse_config(&result).unwrap();
        assert_eq!(
            document["model_providers"]["relay"]["user_option"].as_bool(),
            Some(true)
        );
        assert!(document["model_providers"]["relay"]
            .as_table_like()
            .unwrap()
            .get("base_url")
            .is_none());
    }
}

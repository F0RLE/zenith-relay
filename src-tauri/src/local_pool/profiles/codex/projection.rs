//! The encrypted undo record contains exact before/after documents, not a
//! guessed provider from a rotating diagnostic backup. Only changed TOML
//! leaves and the authentication credential block belong to the attachment.
use super::*;

const AUTH_FIELDS: &[&str] = &["OPENAI_API_KEY", "auth_mode", "tokens", "last_refresh"];

pub(super) fn update_auth_with_rollback(
    auth_path: &Path,
    auth: &Option<Vec<u8>>,
    credential: &str,
    backup: (&Path, &str, &Option<Vec<u8>>),
) -> Result<bool> {
    let update = (|| {
        let updated = merge_auth(snapshot_text(auth, auth_path)?, Some(credential))?
            .ok_or_else(|| LocalPoolError::invalid_state("updated credential is missing"))?;
        replace_if_unchanged(auth_path, auth, &updated)
    })();
    update
        .map(|()| true)
        .map_err(|error| with_rollback(error, rollback_file(backup.0, backup.1, backup.2)))
}

#[derive(Serialize, Deserialize)]
struct Projection {
    version: u32,
    config_before: Option<String>,
    config_after: String,
    auth_before: Option<String>,
}

pub(super) fn save(
    config_before: Option<&str>,
    config_after: &str,
    auth_before: Option<&str>,
    secrets: &impl SecretBackend,
) -> Result<String> {
    let projection = Projection {
        version: 1,
        config_before: config_before.map(str::to_owned),
        config_after: config_after.to_owned(),
        auth_before: auth_before.map(str::to_owned),
    };
    let secret_ref = format!("profile:codex:projection:{}", uuid::Uuid::new_v4());
    secrets.save(
        &secret_ref,
        &serde_json::to_string(&projection).map_err(LocalPoolError::invalid_state)?,
    )?;
    Ok(secret_ref)
}

fn load(secret_ref: &str, secrets: &impl SecretBackend) -> Result<Projection> {
    let content = secrets.load(secret_ref)?.ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Profile undo record is missing",
        )
    })?;
    let projection: Projection = serde_json::from_str(&content).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Profile undo record is invalid",
        )
    })?;
    if projection.version != 1 {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "Unsupported profile undo record version",
        ));
    }
    Ok(projection)
}

pub(super) fn update_websockets(
    secret_ref: &str,
    provider_id: &str,
    enabled: bool,
    secrets: &impl SecretBackend,
) -> Result<()> {
    let mut projection = load(secret_ref, secrets)?;
    let mut after = parse_config(&projection.config_after)?;
    if !set_managed_websockets(&mut after, provider_id, enabled) {
        return Err(profile_restore_blocked());
    }
    // Update only our setting: current user-added fields must never become
    // part of the managed projection and disappear on restore.
    projection.config_after = after.to_string();
    let content = serde_json::to_string(&projection).map_err(LocalPoolError::invalid_state)?;
    secrets.save(secret_ref, &content)
}

pub(super) fn restore(
    secret_ref: &str,
    config: Option<&str>,
    auth: Option<&str>,
    secrets: &impl SecretBackend,
) -> Result<UserProfileSnapshot> {
    let saved = load(secret_ref, secrets)?;
    Ok(UserProfileSnapshot {
        config: restore_config_text(saved.config_before.as_deref(), &saved.config_after, config)?,
        auth: merge_auth(auth, saved.auth_before.as_deref())?,
    })
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
        true,
    )?;
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
    root: bool,
) -> Result<()> {
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
                false,
            )?;
            if previous.is_none() && existing.is_empty() {
                current.remove(&key);
            }
            continue;
        }
        if !same(current.get(&key), managed) {
            // Reasoning is a user preference. A later choice (including
            // deleting it) wins over the temporary attach-time adjustment.
            if root && key == "model_reasoning_effort" {
                continue;
            }
            return Err(profile_restore_blocked());
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
    Ok(())
}

/// Credentials are mutually exclusive; extension fields are not credentials.
/// Callers must verify credential ownership before restoring a saved login.
pub(super) fn merge_auth(
    current: Option<&str>,
    credential: Option<&str>,
) -> Result<Option<String>> {
    let mut current = auth_object(current)?;
    let target = auth_object(credential)?;
    let extensions: serde_json::Map<_, _> = current
        .iter()
        .filter(|(key, _)| !AUTH_FIELDS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let target_extensions: serde_json::Map<_, _> = target
        .iter()
        .filter(|(key, _)| !AUTH_FIELDS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if extensions == target_extensions {
        return Ok(credential.map(str::to_owned));
    }
    for key in AUTH_FIELDS {
        current.remove(*key);
        if let Some(value) = target.get(*key) {
            current.insert((*key).to_owned(), value.clone());
        }
    }
    if current.is_empty() && credential.is_none() {
        return Ok(None);
    }
    serde_json::to_string_pretty(&current)
        .map(|text| Some(format!("{text}\n")))
        .map_err(LocalPoolError::invalid_state)
}

fn auth_object(content: Option<&str>) -> Result<serde_json::Map<String, Value>> {
    let Some(content) = content.filter(|text| !text.trim().is_empty()) else {
        return Ok(Default::default());
    };
    serde_json::from_str::<Value>(content)
        .ok()
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "Profile auth must be a JSON object",
            )
        })
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
    fn changed_managed_endpoint_is_a_conflict() {
        assert!(restore_config_text(
            None,
            "[model_providers.relay]\nbase_url='local'",
            Some("[model_providers.relay]\nbase_url='external'")
        )
        .is_err());
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

    #[test]
    fn auth_switch_preserves_extensions_but_not_old_tokens() {
        let current = r#"{"tokens":{"access_token":"old"},"extension":{"flag":true}}"#;
        let next = merge_auth(
            Some(current),
            Some(r#"{"auth_mode":"apikey","OPENAI_API_KEY":"test"}"#),
        )
        .unwrap()
        .unwrap();
        let next: Value = serde_json::from_str(&next).unwrap();
        assert!(next.get("tokens").is_none());
        assert_eq!(next["extension"]["flag"], true);
    }
}

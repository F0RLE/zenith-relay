use super::*;

const CONTEXT_UNDO_FILE: &str = "codex-context-overrides.json";
const KEYS: [&str; 2] = ["model_context_window", "model_auto_compact_token_limit"];

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ContextUndo {
    profile_dir: String,
    scopes: Vec<ContextScope>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ContextScope {
    profile: Option<String>,
    window: Option<i64>,
    compact: Option<i64>,
    #[serde(default = "previously_managed")]
    window_managed: bool,
    #[serde(default = "previously_managed")]
    compact_managed: bool,
}

fn previously_managed() -> bool {
    true
}

pub(crate) struct ContextConfigChange {
    config_path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
    undo_path: PathBuf,
    undo_before: Option<Vec<u8>>,
    undo_after: Option<Vec<u8>>,
}

impl ContextConfigChange {
    pub(crate) fn rollback(self) -> Result<()> {
        let _guard = lock_codex_profile();
        self.rollback_locked()
    }

    pub(super) fn rollback_locked(self) -> Result<()> {
        if self.after != self.before {
            restore_snapshot_if_unchanged(&self.config_path, &self.after, &self.before)?;
        }
        if self.undo_after != self.undo_before {
            restore_snapshot_if_unchanged(&self.undo_path, &self.undo_after, &self.undo_before)?;
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) fn sync_context_overrides(
    codex_home: &Path,
    backup_root: &Path,
    enabled: bool,
) -> Result<ContextConfigChange> {
    let _guard = lock_codex_profile();
    sync_context_overrides_locked(codex_home, backup_root, enabled)
}

pub(crate) fn sync_context_policy(
    codex_home: &Path,
    backup_root: &Path,
) -> Result<ContextConfigChange> {
    let _guard = lock_codex_profile();
    sync_context_keys_locked(codex_home, backup_root, [true, true])
}

pub(super) fn sync_context_overrides_locked(
    codex_home: &Path,
    backup_root: &Path,
    enabled: bool,
) -> Result<ContextConfigChange> {
    sync_context_keys_locked(codex_home, backup_root, [enabled; 2])
}

fn sync_context_keys_locked(
    codex_home: &Path,
    backup_root: &Path,
    enabled: [bool; 2],
) -> Result<ContextConfigChange> {
    let config_path = codex_home.join(CONFIG_FILE);
    let undo_path = backup_root.join(CONTEXT_UNDO_FILE);
    let before = read_optional_bytes(&config_path)?;
    let undo_before = read_optional_bytes(&undo_path)?;
    if !enabled.iter().any(|value| *value) && undo_before.is_none() {
        return Ok(ContextConfigChange {
            config_path,
            after: before.clone(),
            before,
            undo_path,
            undo_before,
            undo_after: None,
        });
    }
    let mut document = parse_config(snapshot_text(&before, &config_path)?.unwrap_or_default())?;
    let profile_dir = portable_path_string(&canonical_profile_dir(codex_home)?);
    let previous: Option<ContextUndo> = undo_before
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|_| profile_restore_blocked())?;
    if previous
        .as_ref()
        .is_some_and(|undo| undo.profile_dir != profile_dir)
    {
        return Err(profile_restore_blocked());
    }
    let mut undo = previous.unwrap_or_else(|| ContextUndo {
        profile_dir,
        scopes: Vec::new(),
    });
    if enabled.iter().any(|value| *value) {
        // A named profile can override root config. Record only the app's active profile.
        let active_profile = document
            .get("profile")
            .and_then(Item::as_str)
            .map(str::to_owned);
        for profile in std::iter::once(None).chain(active_profile.map(Some)) {
            if !undo.scopes.iter().any(|scope| scope.profile == profile)
                && context_table(&mut document, profile.as_deref()).is_some()
            {
                undo.scopes.push(ContextScope {
                    profile,
                    window: None,
                    compact: None,
                    window_managed: false,
                    compact_managed: false,
                });
            }
        }
    }
    for scope in &mut undo.scopes {
        if let Some(table) = context_table(&mut document, scope.profile.as_deref()) {
            for (index, (saved, managed)) in [
                (&mut scope.window, &mut scope.window_managed),
                (&mut scope.compact, &mut scope.compact_managed),
            ]
            .into_iter()
            .enumerate()
            {
                if !enabled[index] {
                    continue;
                }
                let key = KEYS[index];
                if *managed && table.contains_key(key) {
                    // A newer client override is user-owned, not a failed Relay operation.
                    continue;
                }
                if !*managed {
                    *saved = table
                        .get(key)
                        .map(|item| {
                            item.as_integer().ok_or_else(|| {
                                LocalPoolError::invalid_state("ChatGPT context must be an integer")
                            })
                        })
                        .transpose()?;
                    *managed = true;
                    table.remove(key);
                }
            }
        }
    }
    // Keep both newly acquired and released keys recoverable until config is durable.
    let recovery = serde_json::to_vec(&undo).map_err(LocalPoolError::invalid_state)?;
    for scope in &mut undo.scopes {
        if let Some(table) = context_table(&mut document, scope.profile.as_deref()) {
            for (index, (saved, managed)) in [
                (&mut scope.window, &mut scope.window_managed),
                (&mut scope.compact, &mut scope.compact_managed),
            ]
            .into_iter()
            .enumerate()
            {
                if enabled[index] || !*managed {
                    continue;
                }
                let key = KEYS[index];
                if !table.contains_key(key) {
                    if let Some(number) = *saved {
                        table.insert(key, value(number));
                    }
                }
                *managed = false;
                *saved = None;
            }
        }
    }
    undo.scopes
        .retain(|scope| scope.window_managed || scope.compact_managed);
    let undo_after = if undo.scopes.is_empty() {
        None
    } else {
        Some(serde_json::to_vec(&undo).map_err(LocalPoolError::invalid_state)?)
    };
    let acquiring = enabled.iter().any(|value| *value);
    let intermediate = if acquiring {
        Some(recovery)
    } else {
        undo_before.clone()
    };
    if acquiring {
        fs::create_dir_all(backup_root).map_err(io_error)?;
        let text = std::str::from_utf8(intermediate.as_deref().unwrap_or_default())
            .map_err(LocalPoolError::invalid_state)?;
        replace_if_unchanged(&undo_path, &undo_before, text)?;
    }
    let after = document.to_string();
    if before.as_deref() != Some(after.as_bytes()) {
        if let Err(error) = replace_if_unchanged(&config_path, &before, &after) {
            return Err(with_rollback(
                error,
                restore_snapshot_if_unchanged(&undo_path, &intermediate, &undo_before),
            ));
        }
    }
    let write_final = if let Some(content) = &undo_after {
        let text = std::str::from_utf8(content).map_err(LocalPoolError::invalid_state)?;
        replace_if_unchanged(&undo_path, &intermediate, text)
    } else {
        remove_if_unchanged(&undo_path, &intermediate)
    };
    if let Err(error) = write_final {
        let config_rollback = rollback_file(&config_path, &after, &before);
        let undo_rollback = restore_snapshot_if_unchanged(&undo_path, &intermediate, &undo_before);
        return Err(with_rollback(
            with_rollback(error, config_rollback),
            undo_rollback,
        ));
    }
    Ok(ContextConfigChange {
        config_path,
        before,
        after: Some(after.into_bytes()),
        undo_path,
        undo_before,
        undo_after,
    })
}

fn context_table<'a>(
    document: &'a mut DocumentMut,
    profile: Option<&str>,
) -> Option<&'a mut Table> {
    match profile {
        None => Some(document.as_table_mut()),
        Some(profile) => document
            .get_mut("profiles")?
            .as_table_mut()?
            .get_mut(profile)?
            .as_table_mut(),
    }
}

use super::*;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CodexMemoryStage {
    Extraction,
    Consolidation,
}

impl CodexMemoryStage {
    fn model_key(self) -> &'static str {
        match self {
            Self::Extraction => "extract_model",
            Self::Consolidation => "consolidation_model",
        }
    }
}

fn parse_memory_config(text: &str) -> Result<DocumentMut> {
    // Parser excerpts may contain inline credentials from other config fields.
    parse_config(text).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "ChatGPT config is invalid; repair it before changing the memory model",
        )
    })
}

fn configured_model(document: &DocumentMut, stage: CodexMemoryStage) -> Result<Option<String>> {
    let Some(memories) = document.get("memories") else {
        return Ok(None);
    };
    let memories = memories
        .as_table_like()
        .ok_or_else(|| LocalPoolError::invalid_state("ChatGPT memories config must be a table"))?;
    memories
        .get(stage.model_key())
        .map(|item| {
            item.as_str().map(str::to_owned).ok_or_else(|| {
                LocalPoolError::invalid_state("ChatGPT memory model must be a string")
            })
        })
        .transpose()
}

pub(crate) fn memory_model(codex_home: &Path, stage: CodexMemoryStage) -> Result<Option<String>> {
    let _guard = lock_codex_profile();
    let path = codex_home.join(CONFIG_FILE);
    let before = read_optional_bytes(&path)?;
    let text = snapshot_text(&before, &path)?.unwrap_or_default();
    configured_model(&parse_memory_config(text)?, stage)
}

pub(crate) fn set_memory_model(
    codex_home: &Path,
    backup_root: &Path,
    stage: CodexMemoryStage,
    model: Option<&str>,
    expected_model: Option<&str>,
) -> Result<Option<String>> {
    let _guard = lock_codex_profile();
    let backup_path = backup_path(backup_root);
    let backup_bytes = read_optional_bytes(&backup_path)?;
    let backup = parse_backup_snapshot(&backup_bytes, &backup_path)?.ok_or_else(|| {
        LocalPoolError::invalid_state("Connect ChatGPT to the local Relay pool first")
    })?;
    if backup.credential_kind() != ProfileCredentialKind::LocalGateway || backup.restore_pending {
        return Err(LocalPoolError::invalid_state(
            "Connect ChatGPT to the local Relay pool first",
        ));
    }
    let path = codex_home.join(CONFIG_FILE);
    let before = read_optional_bytes(&path)?;
    let text = snapshot_text(&before, &path)?.unwrap_or_default();
    let mut document = parse_memory_config(text)?;
    if !managed_config_matches(&document, &backup) {
        return Err(profile_changed_at(&path));
    }
    if configured_model(&document, stage)?.as_deref() != expected_model {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "ChatGPT memory model changed; reload before saving",
        ));
    }
    if model == expected_model {
        return Ok(model.map(str::to_owned));
    }
    if let Some(model) = model {
        if document.get("memories").is_none() {
            document["memories"] = Item::Table(Table::new());
        }
        document["memories"][stage.model_key()] = value(model);
    } else if let Some(memories) = document
        .get_mut("memories")
        .and_then(Item::as_table_like_mut)
    {
        memories.remove(stage.model_key());
    }
    replace_if_unchanged(&path, &before, &document.to_string())?;
    Ok(model.map(str::to_owned))
}

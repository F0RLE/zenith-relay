use super::super::{
    io_error_at, parse_config, read_optional_bytes, root_model_catalog_json, snapshot_text,
    CONFIG_FILE,
};
use super::installed::add_installed_codex_ultra;
use super::installed::bundled_codex_ultra_models;
use super::{
    cached_native_catalog_models, catalog_entry_is_picker_eligible, is_native_catalog_entry,
    model_slug, DIRECT_SOURCE_FALLBACK_PRIORITY, MAX_MODEL_CATALOG_BYTES,
};
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};
use zenith_relay_core::{
    codex_catalog_entry_is_compatible, codex_model_is_picker_eligible_for,
    decode_codex_model_alias, normalize_codex_catalog_priorities,
    normalize_native_codex_catalog_entry, normalize_upstream_codex_catalog_entry,
    routed_codex_catalog_entry, CODEX_RELAY_CATALOG_HASH,
};

pub(super) fn collect_native_catalog_template(
    codex_home: &Path,
    user_catalog_path: Option<&str>,
    managed_catalog: Option<&[u8]>,
) -> Result<Option<serde_json::Map<String, Value>>> {
    let mut candidates = Vec::new();
    if let Some(path) = user_catalog_path {
        candidates.extend(read_catalog_file_models(codex_home, path)?);
    }
    candidates.extend(cached_native_catalog_models(codex_home));
    let managed_models = match managed_catalog {
        Some(content) => read_catalog_values(content, false)?,
        None => Vec::new(),
    };
    // Attaching Relay invalidates Codex's live cache after writing a verified
    // catalog. On a later refresh, the current managed catalog is therefore
    // the only remaining compatible schema template. It is never returned as
    // a native model: routed_codex_catalog_entry resets capability fields for
    // a plain upstream /v1/models row before it is advertised again.
    let managed_template = managed_models
        .iter()
        .filter(|catalog_entry| {
            codex_catalog_entry_is_compatible(catalog_entry)
                && catalog_entry_is_picker_eligible(catalog_entry)
        })
        .find_map(Value::as_object)
        .cloned();
    candidates.extend(managed_models);

    let mut models = Vec::new();
    let mut seen = HashSet::new();
    for candidate in candidates {
        if !is_native_catalog_entry(&candidate) || !codex_catalog_entry_is_compatible(&candidate) {
            continue;
        }
        let Some(slug) = model_slug(&candidate) else {
            continue;
        };
        if seen.insert(slug.to_ascii_lowercase()) {
            models.push(candidate);
        }
    }
    let picker_template = |catalog_entry: &&Value| {
        catalog_entry_is_picker_eligible(catalog_entry)
            && catalog_entry.get("supported_in_api") != Some(&Value::Bool(false))
    };
    // Prefer an actual native entry over a namespaced user provider row. The
    // latter remains a useful schema fallback when it is the only catalog
    // available, but must not override native client capabilities by default.
    let template = models
        .iter()
        .filter(|catalog_entry| picker_template(catalog_entry))
        .filter(|catalog_entry| model_slug(catalog_entry).is_some_and(|slug| !slug.contains('/')))
        .find_map(Value::as_object)
        .cloned()
        .or_else(|| {
            models
                .iter()
                .filter(|catalog_entry| picker_template(catalog_entry))
                .find_map(Value::as_object)
                .cloned()
        })
        .or(managed_template);
    Ok(template)
}

pub(super) fn configured_model_catalog_path(codex_home: &Path) -> Result<Option<String>> {
    let config_path = codex_home.join(CONFIG_FILE);
    let config = read_optional_bytes(&config_path)?;
    let document = parse_config(snapshot_text(&config, &config_path)?.unwrap_or_default())?;
    Ok(root_model_catalog_json(&document))
}

fn read_catalog_file_models(codex_home: &Path, configured_path: &str) -> Result<Vec<Value>> {
    let configured_path = Path::new(configured_path);
    let path = if configured_path.is_absolute() {
        configured_path.to_path_buf()
    } else {
        codex_home.join(configured_path)
    };
    // A stale `model_catalog_json` entry must not block a provider switch. The
    // path is retained in the recovery snapshot, while the active Relay
    // catalog can use the native cache or its own validated template. Treat a
    // missing optional source as empty; surface other filesystem failures.
    let content = match fs::read(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error_at(&path, error)),
    };
    read_catalog_values(&content, false)
}

pub(in crate::local_pool::profiles::codex) fn read_catalog_values(
    content: &[u8],
    require_compatible: bool,
) -> Result<Vec<Value>> {
    if content.len() > MAX_MODEL_CATALOG_BYTES {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model catalog exceeds 512 KiB",
        ));
    }
    let catalog_document: Value = serde_json::from_slice(content).map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "ChatGPT model catalog is invalid")
    })?;
    let models = catalog_document
        .get("models")
        .and_then(Value::as_array)
        .filter(|models| !models.is_empty() && models.len() <= 4_096)
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "ChatGPT model catalog has no usable models",
            )
        })?;
    let mut compatible_models = Vec::new();
    for catalog_entry in models {
        if require_compatible && !codex_catalog_entry_is_compatible(catalog_entry) {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "ChatGPT model catalog contains incompatible model entries",
            ));
        }
        if !require_compatible || codex_catalog_entry_is_compatible(catalog_entry) {
            compatible_models.push(catalog_entry.clone());
        }
    }
    Ok(compatible_models)
}

pub(super) fn normalize_model_catalog_values(models: Vec<Value>) -> Result<String> {
    if models.is_empty() || models.len() > 4_096 {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model catalog has no usable models",
        ));
    }
    let mut seen = HashSet::new();
    let mut catalog_entries = models
        .into_iter()
        .filter(codex_catalog_entry_is_compatible)
        .filter(|catalog_entry| {
            model_slug(catalog_entry).is_some_and(|slug| seen.insert(slug.to_ascii_lowercase()))
        })
        .collect::<Vec<_>>();
    if catalog_entries.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model catalog has no compatible models",
        ));
    }
    normalize_codex_catalog_priorities(&mut catalog_entries);
    serde_json::to_string_pretty(&json!({ "models": catalog_entries }))
        .map(|content| format!("{content}\n"))
        .map_err(LocalPoolError::invalid_state)
}

pub(in crate::local_pool::profiles::codex) fn build_managed_model_catalog(
    codex_home: &Path,
    user_catalog_path: Option<&str>,
    current_managed_catalog: Option<&[u8]>,
    relay_catalog_json: &str,
) -> Result<String> {
    let bundled = bundled_codex_ultra_models();
    build_managed_model_catalog_with_bundled(
        codex_home,
        user_catalog_path,
        current_managed_catalog,
        relay_catalog_json,
        &bundled,
    )
}

pub(in crate::local_pool::profiles::codex) fn build_managed_model_catalog_with_bundled(
    codex_home: &Path,
    user_catalog_path: Option<&str>,
    current_managed_catalog: Option<&[u8]>,
    relay_catalog_json: &str,
    bundled: &HashMap<String, Value>,
) -> Result<String> {
    let template =
        collect_native_catalog_template(codex_home, user_catalog_path, current_managed_catalog)?;
    let template = template.unwrap_or_default();
    let relay_models = read_catalog_values(relay_catalog_json.as_bytes(), false)?;
    // The managed provider is the Relay endpoint, so the catalog must contain
    // only models that its live pool exposes. Native/user catalog rows remain
    // untouched in their original profile and only supply a compatible template.
    let mut models = Vec::new();
    let mut seen = HashSet::new();
    let mut accepted = 0usize;
    for (index, relay_model) in relay_models.iter().enumerate() {
        let Some(slug) = model_slug(relay_model) else {
            continue;
        };
        // Direct-source catalogs keep the provider's bare slug for Codex, so
        // the alias prefix alone cannot distinguish them from native rows.
        // The Relay catalog marker is the ownership boundary here.
        let relay_managed = slug.to_ascii_lowercase().starts_with("zenith/")
            || relay_model
                .get("comp_hash")
                .and_then(Value::as_str)
                .is_some_and(|hash| hash == CODEX_RELAY_CATALOG_HASH);
        let model_id = if slug.to_ascii_lowercase().starts_with("zenith/") {
            let Some(decoded_model_id) = decode_codex_model_alias(slug) else {
                continue;
            };
            decoded_model_id
        } else {
            slug.to_string()
        };
        // The Relay catalog already applied the downgrade-id policy.
        if !codex_model_is_picker_eligible_for(&model_id, false) {
            continue;
        }
        accepted += 1;
        let context_window = relay_model
            .get("context_window")
            .and_then(Value::as_u64)
            .filter(|context_window| *context_window > 0);
        let priority = relay_model
            .get("priority")
            .and_then(Value::as_i64)
            .and_then(|priority_value| u64::try_from(priority_value).ok())
            .unwrap_or(DIRECT_SOURCE_FALLBACK_PRIORITY + index as u64);
        // A Relay-owned row may have come from a real upstream Codex catalog.
        // Preserve its strictly validated capability data (including arbitrary
        // reasoning levels) instead of inheriting anything from the native
        // template. Bare rows without the Relay marker are native rows.
        let mut catalog_entry = relay_model
            .as_object()
            .and_then(|upstream| {
                if codex_catalog_entry_is_compatible(relay_model) {
                    // The gateway already projected models.dev capabilities.
                    // Re-normalizing would reintroduce legacy context defaults
                    // and discard output modalities on an unknown model.
                    Some(relay_model.clone())
                } else if relay_managed {
                    normalize_upstream_codex_catalog_entry(
                        upstream,
                        &model_id,
                        priority,
                        context_window,
                    )
                } else {
                    normalize_native_codex_catalog_entry(
                        upstream,
                        &model_id,
                        priority,
                        context_window,
                    )
                }
            })
            .unwrap_or_else(|| {
                if relay_managed {
                    routed_codex_catalog_entry(Some(&template), &model_id, priority, context_window)
                } else {
                    // A malformed native row must not fall back to Relay's
                    // routed context policy. Codex owns native context, so a
                    // missing field stays missing until the native catalog is
                    // available again.
                    let mut fallback =
                        routed_codex_catalog_entry(Some(&template), &model_id, priority, None);
                    if let Some(fallback_object) = fallback.as_object_mut() {
                        for key in [
                            "context_window",
                            "max_context_window",
                            "auto_compact_token_limit",
                            "effective_context_window_percent",
                        ] {
                            fallback_object.remove(key);
                        }
                        fallback_object.insert("slug".into(), Value::String(slug.to_string()));
                    }
                    fallback
                }
            });
        if !slug.to_ascii_lowercase().starts_with("zenith/") {
            catalog_entry["slug"] = Value::String(slug.to_string());
        }
        catalog_entry["comp_hash"] = Value::String(CODEX_RELAY_CATALOG_HASH.into());
        if let Some(display_name) = relay_model.get("display_name").and_then(Value::as_str) {
            catalog_entry["display_name"] = Value::String(display_name.to_string());
        }
        if let Some(description) = relay_model.get("description").and_then(Value::as_str) {
            catalog_entry["description"] = Value::String(description.to_string());
        }
        if relay_managed {
            add_installed_codex_ultra(&mut catalog_entry, &model_id, bundled);
        }
        if let Some(slug) = model_slug(&catalog_entry) {
            if seen.insert(slug.to_ascii_lowercase()) {
                models.push(catalog_entry);
            }
        }
    }
    if accepted == 0 {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "pool has no compatible text models",
        ));
    }
    normalize_model_catalog_values(models)
}

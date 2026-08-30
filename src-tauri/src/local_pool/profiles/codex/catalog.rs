use super::{
    io_error_at, parse_config, read_optional_bytes, root_model_catalog_json, snapshot_text,
    CONFIG_FILE, MODELS_CACHE_FILE,
};
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result};
use serde_json::{json, Value};
use std::{collections::HashSet, fs, path::Path};
use zenith_relay_core::{
    codex_catalog_entry_is_compatible, codex_model_display_name, codex_model_is_picker_eligible,
    decode_codex_model_alias, normalize_codex_catalog_priorities,
    normalize_native_codex_catalog_entry, normalize_upstream_codex_catalog_entry,
    routed_codex_catalog_entry, CODEX_RELAY_CATALOG_HASH,
};

const MAX_MODEL_CATALOG_BYTES: usize = 512 * 1024;
const DIRECT_SOURCE_FALLBACK_PRIORITY: u64 = 1_000;

pub(super) fn direct_source_model_catalog_with_manifest(
    codex_home: &Path,
    source_models: &[String],
    source_manifest: Option<&Value>,
) -> Result<Option<String>> {
    let user_catalog_path = configured_model_catalog_path(codex_home)?;
    let template = collect_native_catalog_template(codex_home, user_catalog_path.as_deref(), None)?;
    // A catalog override is optional in Codex. Relay should prefer a verified
    // native row when one is present, but must not make profile attachment
    // depend on a cache that it deliberately invalidates after catalog changes.
    let template = template.unwrap_or_default();
    // `model_provider` points to this selected source. Native Codex rows are
    // useful only as a schema template here; advertising them would send
    // their requests to this source and produce a false model picker entry.
    let selected_models = source_models
        .iter()
        .map(String::as_str)
        .map(str::trim)
        .filter(|model| is_direct_source_model(model) && codex_model_is_picker_eligible(model))
        .collect::<Vec<_>>();
    let mut models = Vec::new();
    let mut seen = HashSet::new();
    for (index, model) in selected_models.into_iter().enumerate() {
        let normalized = model.to_ascii_lowercase();
        if !seen.insert(normalized) {
            continue;
        }
        let entry = direct_source_catalog_entry(
            &template,
            source_manifest.and_then(|manifest| source_catalog_entry(manifest, model)),
            model,
            DIRECT_SOURCE_FALLBACK_PRIORITY + index as u64,
        );
        if codex_catalog_entry_is_compatible(&entry) {
            models.push(entry);
        }
    }
    if models.is_empty() {
        return Ok(None);
    }
    Ok(Some(normalize_model_catalog_values(models)?))
}

fn is_direct_source_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 256
        && !model.chars().any(char::is_control)
        && !model.to_ascii_lowercase().starts_with("zenith/")
}

pub(super) fn is_native_catalog_entry(entry: &Value) -> bool {
    entry
        .get("slug")
        .and_then(Value::as_str)
        .is_some_and(|slug| {
            !slug.to_ascii_lowercase().starts_with("zenith/")
                && entry
                    .get("comp_hash")
                    .and_then(Value::as_str)
                    .is_none_or(|hash| hash != CODEX_RELAY_CATALOG_HASH)
        })
}

fn cached_native_catalog_models(codex_home: &Path) -> Vec<Value> {
    let Ok(content) = fs::read_to_string(codex_home.join(MODELS_CACHE_FILE)) else {
        return Vec::new();
    };
    let Ok(cache) = serde_json::from_str::<Value>(&content) else {
        return Vec::new();
    };
    cache
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|entry| is_native_catalog_entry(entry))
        .filter(|entry| codex_catalog_entry_is_compatible(entry))
        .cloned()
        .collect()
}

fn model_slug(entry: &Value) -> Option<&str> {
    entry.get("slug").and_then(Value::as_str)
}

fn catalog_entry_is_picker_eligible(entry: &Value) -> bool {
    model_slug(entry).is_some_and(|slug| {
        let model = decode_codex_model_alias(slug).unwrap_or_else(|| slug.to_string());
        codex_model_is_picker_eligible(&model)
    })
}

fn direct_source_catalog_entry(
    template: &serde_json::Map<String, Value>,
    source_entry: Option<&serde_json::Map<String, Value>>,
    model: &str,
    priority: u64,
) -> Value {
    let mut entry = source_entry
        .and_then(|source_entry| {
            normalize_upstream_codex_catalog_entry(source_entry, model, priority, None)
        })
        .unwrap_or_else(|| routed_codex_catalog_entry(Some(template), model, priority, None));
    entry["slug"] = Value::String(model.to_string());
    entry["display_name"] = Value::String(codex_model_display_name(model));
    entry["description"] = Value::String("Available through this API connection.".into());
    entry["comp_hash"] = Value::String(CODEX_RELAY_CATALOG_HASH.into());
    entry
}

fn source_catalog_entry<'a>(
    manifest: &'a Value,
    model: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    manifest
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .find(|entry| {
            entry
                .get("slug")
                .and_then(Value::as_str)
                .is_some_and(|slug| slug.eq_ignore_ascii_case(model))
        })
        .or_else(|| {
            manifest
                .get("data")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
                .find(|entry| {
                    entry
                        .get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| id.eq_ignore_ascii_case(model))
                })
        })
}

fn collect_native_catalog_template(
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
        .filter(|entry| {
            codex_catalog_entry_is_compatible(entry) && catalog_entry_is_picker_eligible(entry)
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
    let picker_template = |entry: &&Value| {
        catalog_entry_is_picker_eligible(entry)
            && entry.get("supported_in_api") != Some(&Value::Bool(false))
    };
    // Prefer an actual native entry over a namespaced user provider row. The
    // latter remains a useful schema fallback when it is the only catalog
    // available, but must not override native client capabilities by default.
    let template = models
        .iter()
        .filter(|entry| picker_template(entry))
        .filter(|entry| model_slug(entry).is_some_and(|slug| !slug.contains('/')))
        .find_map(Value::as_object)
        .cloned()
        .or_else(|| {
            models
                .iter()
                .filter(|entry| picker_template(entry))
                .find_map(Value::as_object)
                .cloned()
        })
        .or(managed_template);
    Ok(template)
}

fn configured_model_catalog_path(codex_home: &Path) -> Result<Option<String>> {
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
    let content = fs::read(&path).map_err(|error| io_error_at(&path, error))?;
    read_catalog_values(&content, false)
}

pub(super) fn read_catalog_values(content: &[u8], require_compatible: bool) -> Result<Vec<Value>> {
    if content.len() > MAX_MODEL_CATALOG_BYTES {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model catalog exceeds 512 KiB",
        ));
    }
    let value: Value = serde_json::from_slice(content).map_err(|_| {
        LocalPoolError::new(ErrorCode::InvalidState, "ChatGPT model catalog is invalid")
    })?;
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .filter(|models| !models.is_empty() && models.len() <= 4_096)
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "ChatGPT model catalog has no usable models",
            )
        })?;
    let mut output = Vec::new();
    for model in models {
        if require_compatible && !codex_catalog_entry_is_compatible(model) {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "ChatGPT model catalog contains incompatible model entries",
            ));
        }
        if !require_compatible || codex_catalog_entry_is_compatible(model) {
            output.push(model.clone());
        }
    }
    Ok(output)
}

fn normalize_model_catalog_values(models: Vec<Value>) -> Result<String> {
    if models.is_empty() || models.len() > 4_096 {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model catalog has no usable models",
        ));
    }
    let mut seen = HashSet::new();
    let mut models = models
        .into_iter()
        .filter(codex_catalog_entry_is_compatible)
        .filter(|model| {
            model_slug(model).is_some_and(|slug| seen.insert(slug.to_ascii_lowercase()))
        })
        .collect::<Vec<_>>();
    if models.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "ChatGPT model catalog has no compatible models",
        ));
    }
    normalize_codex_catalog_priorities(&mut models);
    serde_json::to_string_pretty(&json!({ "models": models }))
        .map(|content| format!("{content}\n"))
        .map_err(LocalPoolError::invalid_state)
}

pub(super) fn build_managed_model_catalog(
    codex_home: &Path,
    user_catalog_path: Option<&str>,
    current_managed_catalog: Option<&[u8]>,
    relay_catalog_json: &str,
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
        let model = if slug.to_ascii_lowercase().starts_with("zenith/") {
            let Some(model) = decode_codex_model_alias(slug) else {
                continue;
            };
            model
        } else {
            slug.to_string()
        };
        if !codex_model_is_picker_eligible(&model) {
            continue;
        }
        accepted += 1;
        let context_window = relay_model
            .get("context_window")
            .and_then(Value::as_u64)
            .filter(|value| *value > 0);
        let priority = relay_model
            .get("priority")
            .and_then(Value::as_i64)
            .and_then(|value| u64::try_from(value).ok())
            .unwrap_or(DIRECT_SOURCE_FALLBACK_PRIORITY + index as u64);
        // A Relay-owned row may have come from a real upstream Codex catalog.
        // Preserve its strictly validated capability data (including arbitrary
        // reasoning levels) instead of inheriting anything from the native
        // template. Bare rows without the Relay marker are native rows.
        let mut entry = relay_model
            .as_object()
            .and_then(|upstream| {
                if relay_managed {
                    normalize_upstream_codex_catalog_entry(
                        upstream,
                        &model,
                        priority,
                        context_window,
                    )
                } else {
                    normalize_native_codex_catalog_entry(upstream, &model, priority, context_window)
                }
            })
            .unwrap_or_else(|| {
                routed_codex_catalog_entry(Some(&template), &model, priority, context_window)
            });
        if !slug.to_ascii_lowercase().starts_with("zenith/") {
            entry["slug"] = Value::String(slug.to_string());
        }
        entry["comp_hash"] = Value::String(CODEX_RELAY_CATALOG_HASH.into());
        if let Some(display_name) = relay_model.get("display_name").and_then(Value::as_str) {
            entry["display_name"] = Value::String(display_name.to_string());
        }
        if let Some(description) = relay_model.get("description").and_then(Value::as_str) {
            entry["description"] = Value::String(description.to_string());
        }
        if let Some(slug) = model_slug(&entry) {
            if seen.insert(slug.to_ascii_lowercase()) {
                models.push(entry);
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

use super::*;

pub(in crate::local_pool::profiles::codex) fn direct_source_model_catalog_with_manifest(
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
        let normalized = zenith_relay_core::model_id_key(model);
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

pub(in crate::local_pool::profiles::codex) fn direct_source_model_catalog_with_capabilities(
    codex_home: &Path,
    source_models: &[String],
    metadata: &ModelMetadataCatalog,
) -> Result<Option<String>> {
    let catalog = direct_source_model_catalog_with_manifest(codex_home, source_models, None)?;
    let Some(catalog) = catalog else {
        return Ok(None);
    };
    let mut value: Value = serde_json::from_str(&catalog)
        .map_err(|_| LocalPoolError::invalid_state("model catalog is invalid"))?;
    let bundled = bundled_codex_ultra_models(codex_home);
    if let Some(models) = value.get_mut("models").and_then(Value::as_array_mut) {
        for model in models {
            let Some(slug) = model.get("slug").and_then(Value::as_str) else {
                continue;
            };
            let decoded = decode_codex_model_alias(slug).unwrap_or_else(|| slug.to_string());
            model["display_name"] = Value::String(metadata.codex_display_name(&decoded));
            metadata.apply_codex_capabilities(&decoded, model);
            add_installed_codex_ultra(model, &decoded, &bundled);
        }
    }
    Ok(Some(
        serde_json::to_string(&value).map_err(LocalPoolError::invalid_state)?,
    ))
}

fn is_direct_source_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 256
        && !model.chars().any(char::is_control)
        && !zenith_relay_core::model_id_key(model).starts_with("zenith/")
}

pub(in crate::local_pool::profiles::codex) fn is_native_catalog_entry(entry: &Value) -> bool {
    entry
        .get("slug")
        .and_then(Value::as_str)
        .is_some_and(|slug| {
            !zenith_relay_core::model_id_key(slug).starts_with("zenith/")
                && entry
                    .get("comp_hash")
                    .and_then(Value::as_str)
                    .is_none_or(|hash| hash != CODEX_RELAY_CATALOG_HASH)
        })
}

pub(super) fn cached_native_catalog_models(codex_home: &Path) -> Vec<Value> {
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

pub(super) fn model_slug(entry: &Value) -> Option<&str> {
    entry.get("slug").and_then(Value::as_str)
}

pub(super) fn catalog_entry_is_picker_eligible(entry: &Value) -> bool {
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

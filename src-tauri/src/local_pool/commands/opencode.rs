use crate::{
    files::atomic_write,
    launcher::restart_opencode,
    local_pool::{
        error::{CommandError, ErrorCode, LocalPoolError},
        models::ProviderSourceRecord,
        state::DesktopState,
        store::secret_store,
    },
    platform::default_opencode_config_path,
};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};
use tauri::State;
use zenith_relay_core::{
    model_metadata::{ModelCapabilities, ModelMetadataCatalog},
    protocol::ModelSummary,
    SourceAdapter, WireApi,
};

const PROVIDER_ID: &str = "zenith-relay";
const MAX_SNAPSHOT_NAME_CHARS: usize = 80;
// OpenCode uses the official OpenAI AI SDK so requests use Relay's Responses
// contract, which preserves tool calls across adapters.
const PROVIDER_NPM: &str = "@ai-sdk/openai";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeConfigStatus {
    pub configured: bool,
    pub model_count: usize,
    pub has_backup: bool,
    pub backup_created_at_ms: Option<u64>,
    pub backup_name: Option<String>,
    pub path: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodeConnectionResult {
    pub path: String,
    pub model_count: usize,
    pub backup_created: bool,
}

fn backup_root(state: &DesktopState) -> PathBuf {
    state.opencode_backup_root()
}

fn backup_path(state: &DesktopState) -> PathBuf {
    backup_root(state).join("original-opencode.json")
}

fn missing_marker_path(state: &DesktopState) -> PathBuf {
    backup_root(state).join("original-opencode.missing")
}

fn backup_name_path(state: &DesktopState) -> PathBuf {
    backup_root(state).join("original-opencode.name")
}

fn normalize_snapshot_name(value: &str) -> Result<String, LocalPoolError> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_SNAPSHOT_NAME_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "OpenCode snapshot name is invalid",
        ));
    }
    Ok(value.to_string())
}

fn backup_name(state: &DesktopState) -> Option<String> {
    fs::read_to_string(backup_name_path(state))
        .ok()
        .and_then(|value| normalize_snapshot_name(&value).ok())
}

fn backup_created_at_ms(state: &DesktopState) -> Option<u64> {
    let path = if backup_path(state).exists() {
        backup_path(state)
    } else if missing_marker_path(state).exists() {
        missing_marker_path(state)
    } else {
        return None;
    };
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
}

fn read_config(path: &Path) -> Result<Map<String, Value>, LocalPoolError> {
    let content = match fs::read_to_string(path) {
        Ok(content) if !content.trim().is_empty() => content,
        Ok(_) => return Ok(Map::new()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(LocalPoolError::new(ErrorCode::Io, error.to_string())),
    };
    let value: Value = parse_jsonc(&content).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            format!("OpenCode config is not valid JSON: {error}"),
        )
    })?;
    value.as_object().cloned().ok_or_else(|| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            "OpenCode config root must be a JSON object",
        )
    })
}

fn write_config(path: &Path, config: &Map<String, Value>) -> Result<(), LocalPoolError> {
    let parent = path.parent().ok_or_else(|| {
        LocalPoolError::new(ErrorCode::Io, "OpenCode config has no parent directory")
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
    let content = serde_json::to_string_pretty(config)
        .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
    atomic_write(path, &format!("{content}\n"))
        .map_err(|error| LocalPoolError::new(ErrorCode::Io, error))
}

fn remove_managed_configuration(config: &mut Map<String, Value>) -> bool {
    let (provider_removed, providers_empty) = config
        .get_mut("provider")
        .and_then(Value::as_object_mut)
        .map_or((false, false), |providers| {
            (
                providers.remove(PROVIDER_ID).is_some(),
                providers.is_empty(),
            )
        });
    if providers_empty {
        config.remove("provider");
    }
    let model_removed = if config
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(|model| model.starts_with(&format!("{PROVIDER_ID}/")))
    {
        config.remove("model");
        true
    } else {
        false
    };
    provider_removed || model_removed
}

fn current_config_is_managed(path: &Path) -> Result<bool, LocalPoolError> {
    if !path.exists() {
        return Ok(true);
    }
    let config = read_config(path)?;
    Ok(config
        .get("provider")
        .and_then(Value::as_object)
        .is_some_and(|providers| providers.contains_key(PROVIDER_ID)))
}

fn restore_original_config_preserving_user_changes(
    original: &Path,
    current: &Path,
) -> Result<(), LocalPoolError> {
    let original_content = fs::read_to_string(original)
        .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
    let mut restored = parse_jsonc(&original_content)
        .map_err(|error| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!("OpenCode snapshot is not valid JSON: {error}"),
            )
        })?
        .as_object()
        .cloned()
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "OpenCode snapshot root must be a JSON object",
            )
        })?;
    let current_config = read_config(current)?;
    for (key, value) in &current_config {
        if key != "provider" && key != "model" {
            restored.insert(key.clone(), value.clone());
        }
    }

    let mut providers = restored
        .get("provider")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if let Some(current_providers) = current_config.get("provider").and_then(Value::as_object) {
        for (id, provider) in current_providers {
            if id != PROVIDER_ID {
                providers.insert(id.clone(), provider.clone());
            }
        }
    }
    if providers.is_empty() {
        restored.remove("provider");
    } else {
        restored.insert("provider".into(), Value::Object(providers));
    }

    let current_model_is_managed = current_config
        .get("model")
        .and_then(Value::as_str)
        .is_some_and(|model| model.starts_with(&format!("{PROVIDER_ID}/")));
    if !current_model_is_managed {
        match current_config.get("model") {
            Some(model) => {
                restored.insert("model".into(), model.clone());
            }
            None => {
                restored.remove("model");
            }
        }
    }
    write_config(current, &restored)
}

/// OpenCode accepts JSONC. Strip comments and trailing commas without
/// touching characters inside JSON strings before handing the value to
/// serde_json. The original bytes remain recoverable through the backup.
fn parse_jsonc(content: &str) -> Result<Value, serde_json::Error> {
    let mut cleaned = String::with_capacity(content.len());
    let mut chars = content.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            cleaned.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            cleaned.push(ch);
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for next in chars.by_ref() {
                if next == '\n' {
                    cleaned.push('\n');
                    break;
                }
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut previous = '\0';
            for next in chars.by_ref() {
                if previous == '*' && next == '/' {
                    break;
                }
                if next == '\n' {
                    cleaned.push('\n');
                }
                previous = next;
            }
            continue;
        }
        cleaned.push(ch);
    }
    let chars = cleaned.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(chars.len());
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in chars.iter().copied().enumerate() {
        if in_string {
            output.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            output.push(ch);
            continue;
        }
        if ch == ',' {
            let mut next = index + 1;
            while next < chars.len() && chars[next].is_whitespace() {
                next += 1;
            }
            if next < chars.len() && (chars[next] == '}' || chars[next] == ']') {
                continue;
            }
        }
        output.push(ch);
    }
    serde_json::from_str(&output)
}

/// Use the canonical management projection consumed by the UI. It already
/// applies pool membership, model rules, hidden models and display order, so
/// integrations cannot drift from Relay's own model catalog.
fn model_ids(models: &[ModelSummary]) -> Vec<ModelSummary> {
    models
        .iter()
        .filter(|model| model.enabled)
        .cloned()
        .collect()
}

fn capability_config(id: &str, capabilities: &ModelCapabilities, levels: &[String]) -> Value {
    let mut value = json!({
        "name": id,
        "attachment": capabilities.attachment.unwrap_or_else(|| capabilities.input_modalities.iter().any(|mode| mode != "text")),
        "reasoning": capabilities.reasoning == Some(true),
        "tool_call": capabilities.tool_call == Some(true),
        "modalities": {"input": capabilities.input_modalities, "output": capabilities.output_modalities}
    });
    if let (Some(context), Some(output)) = (capabilities.context_limit, capabilities.output_limit) {
        value["limit"] = json!({"context": context, "output": output});
        if let Some(input) = capabilities.input_limit {
            value["limit"]["input"] = json!(input);
        }
    }
    if capabilities.reasoning == Some(true) && !levels.is_empty() {
        value["variants"] = Value::Object(
            levels
                .iter()
                .map(|level| (level.clone(), json!({"reasoningEffort": level})))
                .collect(),
        );
    }
    value
}

fn model_config(models: &[ModelSummary]) -> Map<String, Value> {
    models
        .iter()
        .map(|model| {
            let capabilities =
                if model.catalog_input_modalities.is_empty() && model.catalog_provider.is_none() {
                    ModelCapabilities::unknown_model()
                } else {
                    ModelCapabilities {
                        reasoning: model.catalog_reasoning,
                        tool_call: model.catalog_tool_call,
                        attachment: model.catalog_attachment,
                        input_modalities: model.catalog_input_modalities.clone(),
                        output_modalities: model.catalog_output_modalities.clone(),
                        context_limit: model.catalog_context_limit,
                        input_limit: model.catalog_input_limit,
                        output_limit: model.catalog_output_limit,
                        ..ModelCapabilities::default()
                    }
                };
            let supported_levels = if model.reasoning_supported_levels.is_empty() {
                &model.catalog_reasoning_effort_levels
            } else {
                &model.reasoning_supported_levels
            };
            let levels = supported_levels
                .iter()
                .filter(|level| {
                    !model.reasoning_configurable || model.reasoning_allowed_levels.contains(level)
                })
                .cloned()
                .collect::<Vec<_>>();
            (
                model.id.clone(),
                capability_config(&model.id, &capabilities, &levels),
            )
        })
        .collect()
}

fn model_config_ids(models: &[String], metadata: &ModelMetadataCatalog) -> Map<String, Value> {
    models
        .iter()
        .filter(|model| !model.trim().is_empty())
        .map(|model| {
            let capabilities = metadata.capabilities_for(model);
            (
                model.clone(),
                capability_config(model, &capabilities, &capabilities.reasoning_effort_levels),
            )
        })
        .collect()
}

fn managed_provider(base_url: &str, secret: &str, models: &[ModelSummary]) -> Value {
    json!({
        "npm": PROVIDER_NPM,
        "name": "Zenith Relay",
        "options": {
            "baseURL": base_url,
            "apiKey": secret,
        },
        "models": model_config(models),
    })
}

fn managed_provider_for_source(
    base_url: &str,
    secret: &str,
    models: &[String],
    metadata: &ModelMetadataCatalog,
) -> Value {
    json!({
        "npm": PROVIDER_NPM,
        "name": "Zenith Relay",
        "options": {
            "baseURL": base_url,
            "apiKey": secret,
        },
        "models": model_config_ids(models, metadata),
    })
}

fn apply_managed_provider(
    config: &mut Map<String, Value>,
    base_url: &str,
    secret: &str,
    models: &[ModelSummary],
) -> Result<(), LocalPoolError> {
    config
        .entry("$schema")
        .or_insert_with(|| Value::String("https://opencode.ai/config.json".into()));
    config
        .entry("provider")
        .or_insert_with(|| Value::Object(Map::new()));
    let providers = config
        .get_mut("provider")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "OpenCode provider configuration must be an object",
            )
        })?;
    providers.insert(
        PROVIDER_ID.into(),
        managed_provider(base_url, secret, models),
    );

    let current_model = config
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let managed_model_selected = current_model.starts_with(&format!("{PROVIDER_ID}/"));
    if let Some(first) = models.first() {
        if !current_model.contains('/') || managed_model_selected {
            config.insert(
                "model".into(),
                Value::String(format!("{PROVIDER_ID}/{}", first.id)),
            );
        }
    } else if managed_model_selected {
        config.remove("model");
    }
    Ok(())
}

fn backup_original_config(
    state: &DesktopState,
    path: &Path,
    snapshot_name: Option<&str>,
) -> Result<bool, LocalPoolError> {
    let backup = backup_path(state);
    let missing = missing_marker_path(state);
    if backup.exists() || missing.exists() {
        return Ok(false);
    }
    fs::create_dir_all(backup_root(state))
        .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
    if let Some(snapshot_name) = snapshot_name {
        atomic_write(&backup_name_path(state), snapshot_name)
            .map_err(|error| LocalPoolError::new(ErrorCode::Io, error))?;
    }
    if path.exists() {
        fs::copy(path, backup)
            .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
    } else {
        fs::write(missing, b"original config did not exist\n")
            .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
    }
    Ok(true)
}

fn config_status_for(state: &DesktopState) -> Result<OpenCodeConfigStatus, LocalPoolError> {
    let path = default_opencode_config_path();
    let config = read_config(&path)?;
    let provider = config
        .get("provider")
        .and_then(Value::as_object)
        .and_then(|providers| providers.get(PROVIDER_ID));
    let model_count = provider
        .and_then(Value::as_object)
        .and_then(|value| value.get("models"))
        .and_then(Value::as_object)
        .map_or(0, Map::len);
    let has_backup = backup_path(state).exists() || missing_marker_path(state).exists();
    Ok(OpenCodeConfigStatus {
        configured: provider.is_some(),
        model_count,
        has_backup,
        backup_created_at_ms: backup_created_at_ms(state),
        backup_name: has_backup.then(|| backup_name(state)).flatten(),
        path: path.display().to_string(),
    })
}

#[tauri::command]
pub fn get_opencode_config_status(
    state: State<'_, DesktopState>,
) -> Result<OpenCodeConfigStatus, CommandError> {
    config_status_for(&state).map_err(Into::into)
}

/// Preserve the exact OpenCode file before Relay changes it. The operation is
/// intentionally one-shot: an explicit snapshot must never replace the
/// original recovery point created before the first Relay write.
#[tauri::command]
pub async fn create_opencode_snapshot(
    state: State<'_, DesktopState>,
    name: String,
) -> Result<bool, CommandError> {
    let _mutation = state.setup_guard().await;
    let name = normalize_snapshot_name(&name).map_err(CommandError::from)?;
    backup_original_config(&state, &default_opencode_config_path(), Some(&name)).map_err(Into::into)
}

#[tauri::command]
pub async fn connect_opencode_to_local_gateway(
    state: State<'_, DesktopState>,
) -> Result<OpenCodeConnectionResult, CommandError> {
    let _mutation = state.setup_guard().await;
    let path = default_opencode_config_path();
    let prepared = super::state::build_local_runtime_state(&state)
        .await
        .map_err(|error| LocalPoolError::new(error.code, error.message))?;
    let key = super::pool::ensure_system_gateway_key(&state)?;
    if !key.enabled || !super::pool::has_usable_pool_candidate(&state)? {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "managed pool is not available for any enabled candidate",
        )
        .into());
    }
    let secret = super::pool::ensure_local_gateway_key_secret(&key)?;
    let models = model_ids(&prepared.gateway.models);
    let mut config = read_config(&path)?;
    let backup_created = backup_original_config(&state, &path, None)?;
    apply_managed_provider(&mut config, &prepared.gateway.base_url, &secret, &models)?;
    write_config(&path, &config)?;
    Ok(OpenCodeConnectionResult {
        path: path.display().to_string(),
        model_count: models.len(),
        backup_created,
    })
}

/// Configure OpenCode to use one API source directly. This is intentionally
/// separate from the pool connection: selecting a source in the Connections
/// table must not silently fall back to the local pool and must preserve the
/// source's exact endpoint and credential.
#[tauri::command]
pub async fn launch_opencode_source(
    source_id: String,
    state: State<'_, DesktopState>,
) -> Result<OpenCodeConnectionResult, CommandError> {
    let _mutation = state.setup_guard().await;
    let source = state
        .store()?
        .source(&source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    if !source.enabled {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source must be enabled before launching OpenCode",
        )
        .into());
    }
    let models = source_opencode_models(&source)?;
    let secret = secret_store::load(&source.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
    let path = default_opencode_config_path();
    let mut config = read_config(&path)?;
    let backup_created = backup_original_config(&state, &path, None)?;
    let provider = managed_provider_for_source(
        &source.base_url,
        &secret,
        &models,
        &state.model_metadata_catalog(),
    );
    config
        .entry("$schema")
        .or_insert_with(|| Value::String("https://opencode.ai/config.json".into()));
    config
        .entry("provider")
        .or_insert_with(|| Value::Object(Map::new()));
    let providers = config
        .get_mut("provider")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "OpenCode provider configuration must be an object",
            )
        })?;
    providers.insert(PROVIDER_ID.into(), provider);
    config.insert(
        "model".into(),
        Value::String(format!("{PROVIDER_ID}/{}", models[0])),
    );
    write_config(&path, &config)?;
    restart_opencode().map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to restart OpenCode: {error}"),
        )
    })?;
    Ok(OpenCodeConnectionResult {
        path: path.display().to_string(),
        model_count: models.len(),
        backup_created,
    })
}

fn source_opencode_models(source: &ProviderSourceRecord) -> Result<Vec<String>, LocalPoolError> {
    let mut seen = HashSet::new();
    let models = source
        .effective_protocol_bindings()
        .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?
        .into_iter()
        .filter(|binding| {
            binding.wire_api == WireApi::Responses && binding.adapter == SourceAdapter::Native
        })
        .flat_map(|binding| binding.model_ids)
        .filter(|model| !model.trim().is_empty())
        .filter(|model| seen.insert(model.clone()))
        .collect::<Vec<_>>();
    if models.is_empty() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source has no compatible Responses API models",
        ));
    }
    Ok(models)
}

#[tauri::command]
pub fn restart_opencode_app() -> Result<(), CommandError> {
    restart_opencode().map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to restart OpenCode: {error}"),
        )
        .into()
    })
}

#[tauri::command]
pub async fn restore_opencode_config(state: State<'_, DesktopState>) -> Result<bool, CommandError> {
    let _mutation = state.setup_guard().await;
    let path = default_opencode_config_path();
    let backup = backup_path(&state);
    if backup.exists() {
        if !current_config_is_managed(&path)? {
            return Err(LocalPoolError::new(
                ErrorCode::ProfileRestoreBlocked,
                "OpenCode config was changed outside Relay; restore was blocked",
            )
            .into());
        }
        restore_original_config_preserving_user_changes(&backup, &path)?;
        fs::remove_file(&backup)
            .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
        let _ = fs::remove_file(backup_name_path(&state));
        return Ok(true);
    }
    if missing_marker_path(&state).exists() {
        let mut config = read_config(&path)?;
        let managed = remove_managed_configuration(&mut config);
        if managed {
            if config.is_empty() {
                fs::remove_file(&path)
                    .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
            } else {
                write_config(&path, &config)?;
            }
        }
        fs::remove_file(missing_marker_path(&state))
            .map_err(|error| LocalPoolError::new(ErrorCode::Io, error.to_string()))?;
        let _ = fs::remove_file(backup_name_path(&state));
        return Ok(managed);
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_managed_provider, managed_provider, model_ids, normalize_snapshot_name, parse_jsonc,
        remove_managed_configuration, source_opencode_models, ErrorCode, ProviderSourceRecord,
        SourceAdapter, PROVIDER_ID, PROVIDER_NPM,
    };
    use serde_json::{json, Map, Value};
    use std::collections::BTreeMap;
    use zenith_relay_core::{protocol::ModelSummary, SourceProtocolBinding, WireApi};

    fn source(bindings: Vec<SourceProtocolBinding>) -> ProviderSourceRecord {
        ProviderSourceRecord {
            id: "source".into(),
            name: "Source".into(),
            enabled: true,
            in_pool: false,
            draining: false,
            base_url: "https://provider.example/v1".into(),
            secret_ref: "source:test".into(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_bindings: bindings,
            models: vec![
                "gpt-test".into(),
                "gpt-other".into(),
                "chat-only".into(),
                "claude".into(),
            ],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: BTreeMap::new(),
            detected_model_prices: BTreeMap::new(),
            last_used_at: None,
            last_test_at: None,
            last_test_status: None,
            last_error: None,
        }
    }

    fn model(id: &str, enabled: bool) -> ModelSummary {
        ModelSummary {
            id: id.into(),
            enabled,
            member_count: 1,
            codex_visible: enabled,
            codex_display_name: id.into(),
            catalog_provider: None,
            catalog_family: None,
            catalog_name: None,
            catalog_release_date: None,
            catalog_last_updated: None,
            catalog_status: None,
            catalog_reasoning: None,
            catalog_reasoning_method: None,
            catalog_reasoning_effort_levels: Vec::new(),
            catalog_default_reasoning_effort: None,
            catalog_tool_call: None,
            catalog_structured_output: None,
            catalog_attachment: None,
            catalog_open_weights: None,
            catalog_input_modalities: Vec::new(),
            catalog_output_modalities: Vec::new(),
            catalog_context_limit: None,
            catalog_input_limit: None,
            catalog_output_limit: None,
            input_micro_usd_per_million: None,
            cached_input_micro_usd_per_million: None,
            cache_write_5m_micro_usd_per_million: None,
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: None,
            image_request_prices: Vec::new(),
            custom_price: false,
            reasoning_levels: Vec::new(),
            reasoning_supported_levels: Vec::new(),
            reasoning_allowed_levels: Vec::new(),
            reasoning_configurable: false,
            reasoning_manual_fallback: false,
            speed_supported: false,
            speed_tiers: Vec::new(),
            speed_tier: Default::default(),
            speed_configurable: false,
        }
    }

    #[test]
    fn parses_comments_and_trailing_commas_without_changing_strings() {
        let value = parse_jsonc(
            r#"{
                // keep this URL exactly as written
                "url": "https://relay.example/v1//chat",
                "models": ["one", "two",], /* trailing comma */
            }"#,
        )
        .unwrap();
        assert_eq!(value["url"], "https://relay.example/v1//chat");
        assert_eq!(value["models"][1], "two");
    }

    #[test]
    fn keeps_prepared_catalog_order_and_excludes_disabled_models() {
        let models = vec![
            model("claude-opus-4-8", true),
            model("gpt-5.4", false),
            model("gemini-2.5-pro", true),
        ];
        assert_eq!(
            model_ids(&models)
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["claude-opus-4-8", "gemini-2.5-pro"]
        );
    }

    #[test]
    fn configures_opencode_for_responses_tool_calls() {
        let provider = managed_provider(
            "http://127.0.0.1:14998/v1",
            "test-secret",
            &[model("gpt-5.6-sol", true)],
        );

        assert_eq!(provider["npm"], PROVIDER_NPM);
        assert_eq!(provider["options"]["baseURL"], "http://127.0.0.1:14998/v1");
        assert_eq!(provider["models"]["gpt-5.6-sol"]["attachment"], true);
        assert_eq!(
            provider["models"]["gpt-5.6-sol"]["modalities"]["input"],
            json!(["text", "image"])
        );
    }

    #[test]
    fn model_capabilities_are_shared_by_pool_and_direct_source_configs() {
        let metadata = super::ModelMetadataCatalog::from_models_dev_json(
            r#"{
            "test/text-only": {"reasoning": true, "reasoning_effort_levels": ["low", "high"],
            "attachment": false, "tool_call": true,
            "modalities": {"input": ["text"], "output": ["text"]},
            "limit": {"context": 32000, "input": 24000, "output": 8000}}
        }"#,
        )
        .unwrap();
        let mut models = vec![model("text-only", true), model("unknown", true)];
        zenith_relay_core::protocol::apply_model_metadata(&mut models, &metadata);
        let pooled = super::model_config(&models);
        let direct = super::model_config_ids(&["text-only".into(), "unknown".into()], &metadata);
        assert_eq!(pooled, direct);
        assert_eq!(pooled["text-only"]["attachment"], false);
        assert_eq!(pooled["text-only"]["modalities"]["input"], json!(["text"]));
        assert_eq!(pooled["text-only"]["tool_call"], true);
        assert_eq!(pooled["text-only"]["limit"]["context"], 32000);
        assert_eq!(
            pooled["unknown"]["modalities"]["input"],
            json!(["text", "image"])
        );
        assert_eq!(pooled["unknown"]["reasoning"], false);
        assert_eq!(pooled["unknown"]["tool_call"], false);
        assert!(pooled["unknown"].get("limit").is_none());
        models[0].reasoning_configurable = true;
        models[0].reasoning_allowed_levels = vec!["high".into(), "max".into()];
        let filtered = super::model_config(&models);
        assert_eq!(
            filtered["text-only"]["variants"],
            json!({"high": {"reasoningEffort": "high"}})
        );

        models[0].reasoning_supported_levels = vec!["low".into(), "high".into(), "ultra".into()];
        models[0].reasoning_allowed_levels = vec!["high".into(), "ultra".into()];
        let native = super::model_config(&models);
        assert_eq!(
            native["text-only"]["variants"],
            json!({
                "high": {"reasoningEffort": "high"},
                "ultra": {"reasoningEffort": "ultra"}
            })
        );
    }

    #[test]
    fn direct_source_models_are_limited_to_native_responses() {
        let source = source(vec![
            SourceProtocolBinding::legacy(
                WireApi::Responses,
                &["gpt-test".into(), "gpt-other".into(), "gpt-test".into()],
            ),
            SourceProtocolBinding::legacy(WireApi::ChatCompletions, &["chat-only".into()]),
        ]);

        assert_eq!(
            source_opencode_models(&source).unwrap(),
            ["gpt-test", "gpt-other"]
        );
    }

    #[test]
    fn direct_source_models_reject_bridge_only_routes() {
        let mut binding = SourceProtocolBinding::legacy(WireApi::Responses, &["claude".into()]);
        binding.adapter = SourceAdapter::ResponsesToMessages;
        let error = source_opencode_models(&source(vec![binding])).unwrap_err();
        assert_eq!(error.code, ErrorCode::Conflict);
    }

    #[test]
    fn empty_catalog_replaces_stale_models_without_touching_other_providers() {
        let mut config = serde_json::from_value(json!({
            "model": format!("{PROVIDER_ID}/stale-model"),
            "provider": {
                PROVIDER_ID: { "models": { "stale-model": {} } },
                "anthropic": { "name": "Claude" }
            }
        }))
        .unwrap();

        apply_managed_provider(&mut config, "http://127.0.0.1:14998/v1", "test-secret", &[])
            .unwrap();

        assert_eq!(
            config["provider"][PROVIDER_ID]["models"],
            Value::Object(Map::new())
        );
        assert!(config["provider"].get("anthropic").is_some());
        assert!(config.get("model").is_none());
    }

    #[test]
    fn accepts_a_trimmed_snapshot_name_and_rejects_invalid_values() {
        assert_eq!(
            normalize_snapshot_name(" Before switching ").unwrap(),
            "Before switching"
        );
        assert!(normalize_snapshot_name("").is_err());
        assert!(normalize_snapshot_name("bad\nname").is_err());
        assert!(normalize_snapshot_name(&"x".repeat(81)).is_err());
    }

    #[test]
    fn removing_a_missing_original_only_removes_relay_configuration() {
        let mut config = serde_json::from_value(json!({
            "model": format!("{PROVIDER_ID}/gpt-5.6-sol"),
            "provider": {
                PROVIDER_ID: { "name": "Relay" },
                "anthropic": { "name": "Claude" }
            },
            "theme": "dark"
        }))
        .unwrap();

        assert!(remove_managed_configuration(&mut config));
        assert_eq!(config.get("model"), None);
        assert_eq!(config.get("theme"), Some(&json!("dark")));
        assert!(config
            .get("provider")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|providers| providers.contains_key("anthropic")));
        assert!(!config
            .get("provider")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|providers| providers.contains_key(PROVIDER_ID)));
    }
}

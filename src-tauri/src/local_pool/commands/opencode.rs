use crate::{
    files::atomic_write,
    launcher::restart_opencode,
    local_pool::{
        error::{CommandError, ErrorCode, LocalPoolError},
        state::DesktopState,
    },
    platform::default_opencode_config_path,
};
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tauri::State;
use zenith_relay_core::protocol::ModelSummary;

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

fn model_config(models: &[ModelSummary]) -> Map<String, Value> {
    models
        .iter()
        .map(|model| {
            let mut value = Map::new();
            value.insert("name".into(), Value::String(model.id.clone()));
            // Relay accepts image attachments and forwards them to the active
            // route. OpenCode requires both fields before it exposes image
            // attachments for a custom provider model.
            value.insert("attachment".into(), Value::Bool(true));
            value.insert(
                "modalities".into(),
                json!({"input": ["text", "image"], "output": ["text"]}),
            );
            let levels = if !model.reasoning_allowed_levels.is_empty() {
                model.reasoning_allowed_levels.clone()
            } else if !model.reasoning_supported_levels.is_empty() && !model.reasoning_configurable
            {
                model.reasoning_supported_levels.clone()
            } else {
                Vec::new()
            };
            if !levels.is_empty() {
                value.insert("reasoning".into(), Value::Bool(true));
                value.insert(
                    "variants".into(),
                    Value::Object(
                        levels
                            .iter()
                            .map(|level| (level.clone(), json!({ "reasoningEffort": level })))
                            .collect(),
                    ),
                );
            }
            (model.id.clone(), Value::Object(value))
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
    if models.is_empty() {
        return Err(
            LocalPoolError::new(ErrorCode::Conflict, "managed pool has no visible models").into(),
        );
    }
    let mut config = read_config(&path)?;
    let backup_created = backup_original_config(&state, &path, None)?;
    let provider = managed_provider(&prepared.gateway.base_url, &secret, &models);
    config
        .entry("$schema")
        .or_insert_with(|| Value::String("https://opencode.ai/config.json".into()));
    config
        .entry("provider")
        .or_insert_with(|| Value::Object(Map::new()));
    config
        .get_mut("provider")
        .and_then(Value::as_object_mut)
        .unwrap()
        .insert(PROVIDER_ID.into(), provider);
    let current_model = config
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !current_model.contains('/') || current_model.starts_with(&format!("{PROVIDER_ID}/")) {
        config.insert(
            "model".into(),
            Value::String(format!("{PROVIDER_ID}/{}", models[0].id)),
        );
    }
    write_config(&path, &config)?;
    Ok(OpenCodeConnectionResult {
        path: path.display().to_string(),
        model_count: models.len(),
        backup_created,
    })
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
        managed_provider, model_ids, normalize_snapshot_name, parse_jsonc,
        remove_managed_configuration, PROVIDER_ID, PROVIDER_NPM,
    };
    use serde_json::json;
    use zenith_relay_core::protocol::ModelSummary;

    fn model(id: &str, enabled: bool) -> ModelSummary {
        ModelSummary {
            id: id.into(),
            enabled,
            member_count: 1,
            codex_visible: enabled,
            codex_display_name: id.into(),
            catalog_rank: None,
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
            speed_supported: false,
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

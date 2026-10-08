use serde_json::{json, Value};
use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::SystemTime,
};
use zenith_relay_core::apply_codex_ultra_from_official_model;

const MAX_BUNDLED_CODEX_CATALOG_BYTES: usize = 2 * 1024 * 1024;

pub(in crate::local_pool::profiles::codex) fn bundled_codex_ultra_models() -> HashMap<String, Value>
{
    // A desktop session often cannot see `codex` on PATH. Try that command,
    // then the newest installed CLI. Relay only consumes bundled metadata
    // published by Codex itself; another app's catalog must not influence
    // model capabilities.
    codex_cli_candidates()
        .into_iter()
        .find_map(|executable| read_bundled_codex_catalog(&executable))
        .as_ref()
        .map(official_codex_ultra_rows)
        .unwrap_or_default()
}

fn codex_cli_candidates() -> Vec<OsString> {
    let mut candidates = vec![OsString::from("codex")];
    if let Some(bin_dir) = installed_codex_bin_dir() {
        if let Some(executable) = newest_installed_codex_executable(&bin_dir) {
            candidates.push(executable.into_os_string());
        }
    }
    candidates
}

fn installed_codex_bin_dir() -> Option<PathBuf> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(local_app_data)
            .join("OpenAI")
            .join("Codex")
            .join("bin"),
    )
}

pub(super) fn codex_cli_file_name() -> &'static str {
    if cfg!(windows) {
        "codex.exe"
    } else {
        "codex"
    }
}

pub(super) fn newest_installed_codex_executable(bin_dir: &Path) -> Option<PathBuf> {
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for directory_entry in fs::read_dir(bin_dir).ok()?.flatten() {
        let candidate = directory_entry.path().join(codex_cli_file_name());
        let Ok(metadata) = fs::metadata(&candidate) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let replace = match &newest {
            Some((latest_modified_at, latest_catalog_path)) => {
                modified > *latest_modified_at
                    || (modified == *latest_modified_at && candidate > *latest_catalog_path)
            }
            None => true,
        };
        if replace {
            newest = Some((modified, candidate));
        }
    }
    newest.map(|(_, path)| path)
}

fn read_bundled_codex_catalog(executable: &OsString) -> Option<Value> {
    let mut command = Command::new(executable);
    command.args(["debug", "models", "--bundled"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let command_output = command.output().ok()?;
    if !command_output.status.success()
        || command_output.stdout.len() > MAX_BUNDLED_CODEX_CATALOG_BYTES
    {
        return None;
    }
    serde_json::from_slice(&command_output.stdout).ok()
}

pub(super) fn official_codex_ultra_rows(catalog: &Value) -> HashMap<String, Value> {
    catalog
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|catalog_entry| {
            let slug = catalog_entry.get("slug")?.as_str()?;
            let has_ultra = catalog_entry
                .get("supported_reasoning_levels")
                .and_then(Value::as_array)?
                .iter()
                .any(|level| level.get("effort").and_then(Value::as_str) == Some("ultra"));
            if !has_ultra || !zenith_relay_core::is_valid_model_id(slug) {
                return None;
            }
            let mut ultra = json!({
                "slug": slug,
                "supported_reasoning_levels": [{"effort": "ultra"}]
            });
            for field in ["multi_agent_version", "multi_agent_reasoning_effort"] {
                if let Some(value) = catalog_entry.get(field).and_then(Value::as_str) {
                    ultra[field] = json!(value);
                }
            }
            Some((zenith_relay_core::model_id_key(slug), ultra))
        })
        .collect()
}

pub(super) fn add_installed_codex_ultra(
    catalog_entry: &mut Value,
    model: &str,
    bundled: &HashMap<String, Value>,
) {
    if let Some(official) = bundled.get(&zenith_relay_core::model_id_key(model)) {
        apply_codex_ultra_from_official_model(catalog_entry, official, model);
    }
}

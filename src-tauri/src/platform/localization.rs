use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::RwLock;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeMessages {
    pub open: String,
    pub quit: String,
    pub start_pool: String,
    pub stop_pool: String,
    pub pool_running: String,
    pub pool_stopped: String,
    pub sign_in: String,
    pub connected: String,
    pub close_window: String,
}

#[derive(Clone)]
pub struct Presentation {
    pub language: String,
    pub direction: &'static str,
    pub messages: NativeMessages,
}

static PRESENTATION: RwLock<Option<Presentation>> = RwLock::new(None);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SavedLanguage {
    language: String,
    messages: NativeMessages,
}

pub fn initialize(root: &Path) {
    let path = crate::storage_paths::StoragePaths::from_root(root).ui_language_file();
    let Ok(bytes) = crate::files::read_regular_file_bounded(&path, 16 * 1024) else {
        return;
    };
    let Ok(saved) = serde_json::from_slice::<SavedLanguage>(&bytes) else {
        return;
    };
    let _ = set(saved.language, saved.messages);
}

pub fn persist(root: &Path, language: &str, messages: &NativeMessages) -> Result<(), String> {
    validate(language, messages)?;
    let paths = crate::storage_paths::StoragePaths::from_root(root);
    let parent = paths.data_root().join("preferences");
    crate::platform::ensure_real_directory(&parent)?;
    let saved = serde_json::to_string(&SavedLanguage {
        language: language.to_string(),
        messages: messages.clone(),
    })
    .map_err(|_| "Could not encode UI language".to_string())?;
    crate::files::atomic_write(&paths.ui_language_file(), &saved)
        .map_err(|_| "Could not save UI language".to_string())
}

pub fn current() -> Option<Presentation> {
    PRESENTATION.read().ok()?.clone()
}

pub fn validate(language: &str, messages: &NativeMessages) -> Result<&'static str, String> {
    let registry: serde_json::Value =
        serde_json::from_str(include_str!("../../../src/src/i18n/registry.json"))
            .map_err(|_| "Language registry unavailable".to_string())?;
    let entry = registry
        .as_array()
        .and_then(|entries| entries.iter().find(|entry| entry["code"] == language))
        .ok_or_else(|| "Unsupported UI language".to_string())?;
    for text in [
        &messages.open,
        &messages.quit,
        &messages.start_pool,
        &messages.stop_pool,
        &messages.pool_running,
        &messages.pool_stopped,
        &messages.sign_in,
        &messages.connected,
        &messages.close_window,
    ] {
        if text.trim().is_empty() || text.len() > 1024 || text.chars().any(char::is_control) {
            return Err("Invalid UI translation".to_string());
        }
    }
    match entry["direction"].as_str() {
        Some("ltr") => Ok("ltr"),
        Some("rtl") => Ok("rtl"),
        _ => Err("Invalid UI language direction".to_string()),
    }
}

pub fn set(language: String, messages: NativeMessages) -> Result<(), String> {
    let direction = validate(&language, &messages)?;
    *PRESENTATION
        .write()
        .map_err(|_| "UI language unavailable".to_string())? = Some(Presentation {
        language,
        direction,
        messages,
    });
    Ok(())
}

pub fn text(select: impl FnOnce(&NativeMessages) -> &str, fallback: &str) -> String {
    current().map_or_else(
        || fallback.to_string(),
        |presentation| select(&presentation.messages).to_string(),
    )
}

pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages() -> NativeMessages {
        NativeMessages {
            open: "Open".into(),
            quit: "Quit".into(),
            start_pool: "Start".into(),
            stop_pool: "Stop".into(),
            pool_running: "Running".into(),
            pool_stopped: "Stopped".into(),
            sign_in: "Sign in".into(),
            connected: "Connected".into(),
            close_window: "Close".into(),
        }
    }

    #[test]
    fn locale_and_copy_are_bounded_before_mutation() {
        assert_eq!(validate("en", &messages()), Ok("ltr"));
        assert!(validate("../../escape", &messages()).is_err());
        let mut invalid = messages();
        invalid.quit = "bad\nlabel".into();
        assert!(validate("en", &invalid).is_err());
        invalid.quit = "x".repeat(1025);
        assert!(validate("en", &invalid).is_err());
    }

    #[test]
    fn translated_callback_copy_is_escaped() {
        assert_eq!(
            escape_html("<script>\"&'"),
            "&lt;script&gt;&quot;&amp;&#39;"
        );
    }

    #[test]
    fn preferences_round_trip_and_reject_invalid_replacements() {
        let root = crate::files::test_temp_root("language-roundtrip");
        persist(&root, "en", &messages()).unwrap();
        let path = crate::storage_paths::StoragePaths::from_root(&root).ui_language_file();
        let original = std::fs::read(&path).unwrap();
        let saved: SavedLanguage = serde_json::from_slice(&original).unwrap();
        assert_eq!(saved.language, "en");
        assert_eq!(saved.messages.quit, "Quit");
        assert_eq!(validate(&saved.language, &saved.messages), Ok("ltr"));
        let mut invalid = messages();
        invalid.quit.clear();
        assert!(persist(&root, "en", &invalid).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unavailable_preferences_report_an_error_without_overwriting_the_blocker() {
        let root = crate::files::test_temp_root("language-write-blocker");
        let blocker = root.join("data");
        std::fs::write(&blocker, "synthetic existing file").unwrap();
        assert!(persist(&root, "en", &messages()).is_err());
        assert_eq!(
            std::fs::read_to_string(&blocker).unwrap(),
            "synthetic existing file"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

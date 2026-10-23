use crate::platform::{platform_name, system_locale};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub(super) fn get_platform() -> &'static str {
    platform_name()
}

#[tauri::command]
pub(super) fn get_system_locale() -> Option<String> {
    system_locale()
}

#[tauri::command]
pub(super) fn open_api_key_page(provider: String, app: AppHandle) -> Result<(), String> {
    let url = api_key_page_url(&provider).ok_or_else(|| "Unsupported API provider.".to_string())?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|error| error.to_string())
}

fn api_key_page_url(provider: &str) -> Option<&'static str> {
    match provider {
        "zenith" => Some("https://t.me/zenith_service_bot"),
        "openai" => Some("https://platform.openai.com/api-keys"),
        "openrouter" => Some("https://openrouter.ai/settings/keys"),
        "anthropic" => Some("https://platform.claude.com/settings/keys"),
        "gemini" => Some("https://aistudio.google.com/app/apikey"),
        "deepseek" => Some("https://platform.deepseek.com/api_keys"),
        "groq" => Some("https://console.groq.com/keys"),
        "mistral" => Some("https://console.mistral.ai/"),
        "moonshot" => Some("https://platform.kimi.ai/console/api-keys"),
        "kimi" => Some("https://www.kimi.com/code/console"),
        "minimax" => Some("https://platform.minimax.io/console/access"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::api_key_page_url;

    #[test]
    fn api_key_pages_are_fixed_to_known_providers() {
        assert_eq!(
            api_key_page_url("zenith"),
            Some("https://t.me/zenith_service_bot")
        );
        assert_eq!(
            api_key_page_url("openai"),
            Some("https://platform.openai.com/api-keys")
        );
        assert_eq!(
            api_key_page_url("openrouter"),
            Some("https://openrouter.ai/settings/keys")
        );
        assert_eq!(api_key_page_url("custom"), None);
    }
}

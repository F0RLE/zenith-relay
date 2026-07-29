use super::top_up;
use crate::{codex_config::load_api_key_for_launch, key_storage::load_saved_app_key};
use serde_json::Value;
use url::Url;

const DEFAULT_API_BASE_URL: &str = "https://api.zenithmarket.dev/v1";

pub(super) fn api_url(path: &str) -> String {
    format!("{}{}", DEFAULT_API_BASE_URL.trim_end_matches('/'), path)
}

pub(super) async fn api_get(
    client: &reqwest::Client,
    path: &str,
    api_key: &str,
) -> Result<reqwest::Response, String> {
    client
        .get(api_url(path))
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|err| format!("API request failed: {err}"))
}

pub(super) fn normalize_api_key(api_key: &str) -> Result<String, String> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err("API key is required.".to_string());
    }
    Ok(api_key.to_string())
}

pub(super) fn stored_api_key() -> Result<String, String> {
    let api_key = load_saved_app_key()
        .or_else(load_api_key_for_launch)
        .ok_or_else(|| "API key is not configured.".to_string())?;
    normalize_api_key(&api_key)
}

pub(super) async fn api_error_message(response: reqwest::Response, fallback: &str) -> String {
    let status = response.status();
    let raw_message = match response.json::<Value>().await {
        Ok(payload) => payload
            .get("error")
            .and_then(|error| {
                error.as_str().map(str::to_string).or_else(|| {
                    error
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
            })
            .or_else(|| {
                payload
                    .get("message")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| fallback.to_string()),
        Err(_) => fallback.to_string(),
    };
    let message = sanitize_api_error_message(&raw_message, fallback);
    format!("{message} ({})", status.as_u16())
}

pub(super) fn sanitize_api_error_message(message: &str, fallback: &str) -> String {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    let lower = trimmed.to_ascii_lowercase();
    let sensitive_non_url_markers = [
        "znt_",
        "zrk_",
        "sk-",
        "bearer ",
        "authorization",
        "api key",
        "token",
        "provider",
        "upstream",
        "cf-ray",
        "cloudflare",
    ];
    if sensitive_non_url_markers
        .iter()
        .any(|marker| lower.contains(marker))
    {
        return fallback.to_string();
    }
    let url_markers = ["http://", "https://", "tg://"];
    if url_markers.iter().any(|marker| lower.contains(marker))
        && !contains_only_safe_public_support_links(trimmed)
    {
        return fallback.to_string();
    }
    trimmed.chars().take(240).collect()
}

pub(super) fn contains_only_safe_public_support_links(message: &str) -> bool {
    for word in message.split_whitespace() {
        let candidate = word.trim_matches(|character: char| {
            matches!(character, '.' | ',' | ';' | ':' | ')' | ']' | '}')
        });
        if (candidate.starts_with("http://")
            || candidate.starts_with("https://")
            || candidate.starts_with("tg://"))
            && !is_safe_public_support_link(candidate)
        {
            return false;
        }
    }
    true
}

pub(super) fn is_safe_public_support_link(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    if url.scheme() == "https" && url.host_str() == Some("t.me") {
        return url.path() == "/zenith_service_bot";
    }
    if url.scheme() == "tg" && url.host_str() == Some("resolve") {
        return url
            .query_pairs()
            .any(|(key, value)| key == "domain" && value == top_up::BOT_DOMAIN);
    }
    false
}

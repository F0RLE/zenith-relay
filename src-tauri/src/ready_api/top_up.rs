use super::{
    client::{api_error_message, api_url, normalize_api_key, stored_api_key},
    models::{ApiEnvelope, PreparedTopUpAmount, TopUpIntentData},
};
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;
use url::Url;

pub(super) const BOT_URL: &str = "https://t.me/zenith_service_bot";
pub(super) const BOT_DOMAIN: &str = "zenith_service_bot";
pub(super) const MAX_AMOUNT_CENTS: i64 = 1_000_000;

#[tauri::command]
pub(super) fn prepare_top_up_amount(amount_text: String) -> PreparedTopUpAmount {
    match parse_usd_amount(&amount_text) {
        Some(amount_usd) => PreparedTopUpAmount {
            amount_cents: (amount_usd * 100.0).round() as i64,
            amount_usd,
            valid: true,
        },
        None => PreparedTopUpAmount {
            amount_cents: 0,
            amount_usd: 0.0,
            valid: false,
        },
    }
}

#[tauri::command]
pub(super) async fn create_top_up_intent_and_open(
    api_key: String,
    amount_cents: i64,
    app: AppHandle,
) -> Result<(), String> {
    let api_key = normalize_api_key(&api_key)?;
    create_top_up_intent(&api_key, amount_cents, app).await
}

#[tauri::command]
pub(super) async fn create_saved_top_up_intent_and_open(
    amount_cents: i64,
    app: AppHandle,
) -> Result<(), String> {
    let api_key = stored_api_key()?;
    create_top_up_intent(&api_key, amount_cents, app).await
}

async fn create_top_up_intent(
    api_key: &str,
    amount_cents: i64,
    app: AppHandle,
) -> Result<(), String> {
    validate_top_up_amount_cents(amount_cents)?;
    let top_up_response = reqwest::Client::new()
        .post(api_url("/desktop/top-up-intents"))
        .bearer_auth(api_key)
        .json(&serde_json::json!({ "amountCents": amount_cents }))
        .send()
        .await
        .map_err(|err| format!("Could not create a top-up intent: {err}"))?;
    if !top_up_response.status().is_success() {
        return Err(api_error_message(top_up_response, "Could not create a top-up intent.").await);
    }
    let top_up_envelope = top_up_response
        .json::<ApiEnvelope<TopUpIntentData>>()
        .await
        .map_err(|err| format!("Top-up intent response is invalid: {err}"))?;
    let start = extract_top_up_start(top_up_envelope.data)
        .ok_or_else(|| "Top-up intent response is missing a start payload.".to_string())?;
    open_top_up_url(telegram_start_url(&start), app)
}

#[tauri::command]
pub(super) fn open_top_up_url(url: String, app: AppHandle) -> Result<(), String> {
    if !is_allowed_top_up_url(&url) {
        return Err("Unsupported top-up URL.".to_string());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|err| err.to_string())
}

pub(super) fn is_allowed_top_up_url(url_text: &str) -> bool {
    let Ok(parsed_url) = Url::parse(url_text) else {
        return false;
    };
    if parsed_url.scheme() != "tg" || parsed_url.host_str() != Some("resolve") {
        return false;
    }
    let mut has_start = false;
    let mut has_domain = false;
    for (query_key, query_value) in parsed_url.query_pairs() {
        if query_key == "domain" && query_value == BOT_DOMAIN {
            has_domain = true;
        }
        if query_key == "start" && !query_value.is_empty() {
            has_start = true;
        }
    }
    has_domain && has_start && parsed_url.fragment().is_none()
}

fn parse_usd_amount(amount_text: &str) -> Option<f64> {
    let trimmed = amount_text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let comma_count = trimmed.matches(',').count();
    let normalized = if comma_count > 1 || looks_like_grouped_decimal(trimmed) {
        trimmed.replace(',', "")
    } else {
        trimmed.replace(',', ".")
    };
    let amount = normalized.parse::<f64>().ok()?;
    if !amount.is_finite() || !(1.0..=10_000.0).contains(&amount) {
        return None;
    }
    Some((amount * 100.0).round() / 100.0)
}

pub(super) fn validate_top_up_amount_cents(amount_cents: i64) -> Result<(), String> {
    if amount_cents <= 0 {
        return Err("Top-up amount must be positive.".to_string());
    }
    if amount_cents > MAX_AMOUNT_CENTS {
        return Err("Top-up amount is too large.".to_string());
    }
    Ok(())
}

fn looks_like_grouped_decimal(amount_text: &str) -> bool {
    amount_text
        .split_once(',')
        .map(|(_, tail)| tail.chars().take_while(|ch| ch.is_ascii_digit()).count() == 3)
        .unwrap_or(false)
}

pub(super) fn extract_top_up_start(intent_data: TopUpIntentData) -> Option<String> {
    let TopUpIntentData {
        bot_url,
        url,
        start_parameter,
        start_payload,
        code,
    } = intent_data;
    bot_url
        .as_deref()
        .and_then(extract_top_up_start_from_url)
        .or_else(|| url.as_deref().and_then(extract_top_up_start_from_url))
        .or_else(|| start_parameter.filter(|start| is_valid_top_up_start(start)))
        .or_else(|| start_payload.filter(|start| is_valid_top_up_start(start)))
        .or_else(|| code.filter(|start| is_valid_top_up_start(start)))
}

pub(super) fn extract_top_up_start_from_url(url_text: &str) -> Option<String> {
    let parsed_url = Url::parse(url_text).ok()?;
    if parsed_url.scheme() == "tg"
        && parsed_url.host_str() == Some("resolve")
        && parsed_url
            .query_pairs()
            .any(|(key, query_value)| key == "domain" && query_value == BOT_DOMAIN)
    {
        return parsed_url
            .query_pairs()
            .find_map(|(key, query_value)| (key == "start").then(|| query_value.to_string()))
            .filter(|start| is_valid_top_up_start(start));
    }
    let base = Url::parse(BOT_URL).ok()?;
    if parsed_url.scheme() == base.scheme()
        && parsed_url.host_str() == base.host_str()
        && parsed_url.path() == base.path()
    {
        return parsed_url
            .query_pairs()
            .find_map(|(key, query_value)| (key == "start").then(|| query_value.to_string()))
            .filter(|start| is_valid_top_up_start(start));
    }
    None
}

pub(super) fn telegram_start_url(start: &str) -> String {
    let mut url = Url::parse("tg://resolve").expect("static tg URL is valid");
    url.query_pairs_mut()
        .append_pair("domain", BOT_DOMAIN)
        .append_pair("start", start);
    url.to_string()
}

fn is_valid_top_up_start(start: &str) -> bool {
    let Some(rest) = start.strip_prefix("ztu_") else {
        return false;
    };
    rest.len() == 36
        && rest
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

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
pub(super) fn prepare_top_up_amount(value: String) -> PreparedTopUpAmount {
    match parse_usd_amount(&value) {
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
    let response = reqwest::Client::new()
        .post(api_url("/desktop/top-up-intents"))
        .bearer_auth(api_key)
        .json(&serde_json::json!({ "amountCents": amount_cents }))
        .send()
        .await
        .map_err(|err| format!("Could not create a top-up intent: {err}"))?;
    if !response.status().is_success() {
        return Err(api_error_message(response, "Could not create a top-up intent.").await);
    }
    let payload = response
        .json::<ApiEnvelope<TopUpIntentData>>()
        .await
        .map_err(|err| format!("Top-up intent response is invalid: {err}"))?;
    let start = extract_top_up_start(payload.data)
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

pub(super) fn is_allowed_top_up_url(value: &str) -> bool {
    let Ok(input) = Url::parse(value) else {
        return false;
    };
    if input.scheme() != "tg" || input.host_str() != Some("resolve") {
        return false;
    }
    let mut has_start = false;
    let mut has_domain = false;
    for (key, value) in input.query_pairs() {
        if key == "domain" && value == BOT_DOMAIN {
            has_domain = true;
        }
        if key == "start" && !value.is_empty() {
            has_start = true;
        }
    }
    has_domain && has_start && input.fragment().is_none()
}

fn parse_usd_amount(value: &str) -> Option<f64> {
    let trimmed = value.trim();
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

fn looks_like_grouped_decimal(value: &str) -> bool {
    value
        .split_once(',')
        .map(|(_, tail)| tail.chars().take_while(|ch| ch.is_ascii_digit()).count() == 3)
        .unwrap_or(false)
}

pub(super) fn extract_top_up_start(data: TopUpIntentData) -> Option<String> {
    let TopUpIntentData {
        bot_url,
        url,
        start_parameter,
        start_payload,
        code,
    } = data;
    bot_url
        .as_deref()
        .and_then(extract_top_up_start_from_url)
        .or_else(|| url.as_deref().and_then(extract_top_up_start_from_url))
        .or_else(|| start_parameter.filter(|start| is_valid_top_up_start(start)))
        .or_else(|| start_payload.filter(|start| is_valid_top_up_start(start)))
        .or_else(|| code.filter(|start| is_valid_top_up_start(start)))
}

pub(super) fn extract_top_up_start_from_url(value: &str) -> Option<String> {
    let input = Url::parse(value).ok()?;
    if input.scheme() == "tg"
        && input.host_str() == Some("resolve")
        && input
            .query_pairs()
            .any(|(key, value)| key == "domain" && value == BOT_DOMAIN)
    {
        return input
            .query_pairs()
            .find_map(|(key, value)| (key == "start").then(|| value.to_string()))
            .filter(|start| is_valid_top_up_start(start));
    }
    let base = Url::parse(BOT_URL).ok()?;
    if input.scheme() == base.scheme()
        && input.host_str() == base.host_str()
        && input.path() == base.path()
    {
        return input
            .query_pairs()
            .find_map(|(key, value)| (key == "start").then(|| value.to_string()))
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

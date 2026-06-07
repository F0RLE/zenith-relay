#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod codex_config;
mod files;
mod key_storage;
mod launcher;
mod platform;
mod tray;

use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    env,
    io::Write,
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_opener::OpenerExt;
use url::Url;

use crate::{
    codex_config::{
        enable_provider, ensure_provider_on_launch, load_api_key_for_launch, provider_has_token,
        reset_provider,
    },
    key_storage::{load_saved_app_key, save_app_key},
    launcher::{is_codex_running, launch_codex, restart_codex_if_running},
    platform::{platform_name, system_locale},
    tray::{build_tray, close_main_window, AppState},
};

const SINGLE_INSTANCE_ADDR: &str = "127.0.0.1:47831";
const DEFAULT_API_BASE_URL: &str = "https://api.zenithmarket.dev/v1";
const TOP_UP_BOT_URL: &str = "https://t.me/zenith_service_bot";
const TOP_UP_BOT_DOMAIN: &str = "zenith_service_bot";
const MAX_TOP_UP_AMOUNT_CENTS: i64 = 1_000_000;
const USAGE_HISTORY_LIMIT: u8 = 8;
const STATS_WATCH_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UiState {
    provider_active: bool,
    codex_running: bool,
    saved_api_key: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KeyStats {
    masked_key: String,
    label: Option<String>,
    enabled: bool,
    status: String,
    balance_cents: i64,
    #[serde(default)]
    balance_microusd: Option<i64>,
    spent_cents: i64,
    #[serde(default)]
    spent_microusd: Option<i64>,
    total_credits_cents: i64,
    #[serde(default)]
    total_credits_microusd: Option<i64>,
    requests: i64,
    input_tokens: i64,
    cached_input_tokens: i64,
    reasoning_tokens: i64,
    output_tokens: i64,
    total_tokens: i64,
    daily_spent_cents: i64,
    #[serde(default)]
    daily_spent_microusd: Option<i64>,
    weekly_spent_cents: i64,
    #[serde(default)]
    weekly_spent_microusd: Option<i64>,
    monthly_spent_cents: i64,
    #[serde(default)]
    monthly_spent_microusd: Option<i64>,
    #[serde(default)]
    balance: String,
    #[serde(default)]
    spent: String,
    #[serde(default)]
    total_credits: String,
    #[serde(default)]
    requests_display: String,
    #[serde(default)]
    input_tokens_display: String,
    #[serde(default)]
    cached_input_tokens_display: String,
    #[serde(default)]
    reasoning_tokens_display: String,
    #[serde(default)]
    output_tokens_display: String,
    #[serde(default)]
    total_tokens_display: String,
    #[serde(default)]
    daily_spent: String,
    #[serde(default)]
    weekly_spent: String,
    #[serde(default)]
    monthly_spent: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageLogEntry {
    id: i64,
    model: Option<String>,
    request_id: Option<String>,
    input_tokens: i64,
    cached_input_tokens: i64,
    reasoning_tokens: i64,
    output_tokens: i64,
    total_tokens: i64,
    cost_cents: i64,
    #[serde(default)]
    cost_microusd: Option<i64>,
    status: String,
    created_at: String,
    #[serde(default)]
    model_display: String,
    #[serde(default)]
    created_at_display: String,
    #[serde(default)]
    cost: String,
    #[serde(default)]
    input_tokens_display: String,
    #[serde(default)]
    cached_input_tokens_display: String,
    #[serde(default)]
    reasoning_tokens_display: String,
    #[serde(default)]
    output_tokens_display: String,
    #[serde(default)]
    total_tokens_display: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageLogPage {
    usage: Vec<UsageLogEntry>,
    limit: i64,
    since_id: Option<i64>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageVersion {
    version: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TopUpIntentData {
    bot_url: Option<String>,
    url: Option<String>,
    start_parameter: Option<String>,
    start_payload: Option<String>,
    code: Option<String>,
}

#[derive(Deserialize)]
struct ApiEnvelope<T> {
    data: T,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreparedTopUpAmount {
    amount_cents: i64,
    amount_usd: f64,
    valid: bool,
}

#[tauri::command]
fn get_state() -> UiState {
    let _ = ensure_provider_on_launch();
    UiState {
        provider_active: provider_has_token(),
        codex_running: is_codex_running(),
        saved_api_key: load_saved_app_key()
            .or_else(load_api_key_for_launch)
            .unwrap_or_default(),
    }
}

#[tauri::command]
fn get_platform() -> &'static str {
    platform_name()
}

#[tauri::command]
fn get_system_locale() -> Option<String> {
    system_locale()
}

#[tauri::command]
async fn get_key_stats(api_key: String) -> Result<KeyStats, String> {
    let api_key = normalize_api_key(&api_key)?;
    fetch_key_stats(&api_key).await
}

async fn fetch_key_stats(api_key: &str) -> Result<KeyStats, String> {
    let client = reqwest::Client::new();
    let response = api_get(&client, "/zenith/key/stats", api_key).await?;
    if !response.status().is_success() {
        return Err(api_error_message(response, "Stats request failed.").await);
    }

    let payload = response
        .json::<ApiEnvelope<Value>>()
        .await
        .map_err(|err| format!("Stats response is invalid: {err}"))?;
    let mut stats = key_stats_from_value(&payload.data);
    enrich_key_stats(&mut stats);
    Ok(stats)
}

#[tauri::command]
async fn get_key_usage_history(
    api_key: String,
    since_id: Option<i64>,
    after_id: Option<i64>,
) -> Result<UsageLogPage, String> {
    let api_key = normalize_api_key(&api_key)?;
    fetch_key_usage_history(&api_key, since_id, after_id).await
}

async fn fetch_key_usage_history(
    api_key: &str,
    since_id: Option<i64>,
    after_id: Option<i64>,
) -> Result<UsageLogPage, String> {
    let mut path = format!("/zenith/key/usage?limit={USAGE_HISTORY_LIMIT}");
    if let Some(after_id) = after_id {
        if after_id > 0 {
            path.push_str("&afterId=");
            path.push_str(&after_id.to_string());
        }
    }
    if let Some(since_id) = since_id {
        if since_id > 0 {
            path.push_str("&sinceId=");
            path.push_str(&since_id.to_string());
        }
    }

    let client = reqwest::Client::new();
    let response = api_get(&client, &path, api_key).await?;
    if !response.status().is_success() {
        return Err(api_error_message(response, "Usage history request failed.").await);
    }

    let mut payload = response
        .json::<ApiEnvelope<UsageLogPage>>()
        .await
        .map_err(|err| format!("Usage history response is invalid: {err}"))?;
    for entry in &mut payload.data.usage {
        enrich_usage_log_entry(entry);
    }
    Ok(payload.data)
}

#[tauri::command]
async fn get_key_usage_version(api_key: String) -> Result<UsageVersion, String> {
    let api_key = normalize_api_key(&api_key)?;
    fetch_key_usage_version(&api_key).await
}

async fn fetch_key_usage_version(api_key: &str) -> Result<UsageVersion, String> {
    let client = reqwest::Client::new();
    let response = api_get(&client, "/zenith/key/usage-version", api_key).await?;
    if !response.status().is_success() {
        return Err(api_error_message(response, "Usage version request failed.").await);
    }

    let payload = response
        .json::<ApiEnvelope<UsageVersion>>()
        .await
        .map_err(|err| format!("Usage version response is invalid: {err}"))?;
    Ok(payload.data)
}

fn start_key_stats_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last_key = String::new();
        let mut last_usage_version: Option<i64> = None;

        loop {
            tokio::time::sleep(STATS_WATCH_INTERVAL).await;

            if app.get_webview_window("main").is_none() {
                continue;
            }

            let api_key = load_saved_app_key()
                .or_else(load_api_key_for_launch)
                .unwrap_or_default();
            let api_key = api_key.trim().to_string();
            if api_key.is_empty() {
                last_key.clear();
                last_usage_version = None;
                continue;
            }
            if api_key != last_key {
                last_key = api_key.clone();
                last_usage_version = None;
            }

            if let Ok(stats) = fetch_key_stats(&api_key).await {
                let _ = app.emit("zenith-key-stats-changed", stats);
            }

            let Ok(version) = fetch_key_usage_version(&api_key).await else {
                continue;
            };
            let previous = last_usage_version;
            last_usage_version = Some(version.version);

            if let Some(previous) = previous {
                if version.version != previous {
                    if let Ok(page) = fetch_key_usage_history(&api_key, None, Some(previous)).await
                    {
                        let _ = app.emit("zenith-usage-history-changed", page);
                    }
                }
            }
        }
    });
}

#[tauri::command]
fn prepare_top_up_amount(value: String) -> PreparedTopUpAmount {
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
async fn create_top_up_intent_and_open(
    api_key: String,
    amount_cents: i64,
    app: AppHandle,
) -> Result<(), String> {
    let api_key = normalize_api_key(&api_key)?;
    validate_top_up_amount_cents(amount_cents)?;

    let client = reqwest::Client::new();
    let response = client
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
    let url = telegram_start_url(&start);
    open_top_up_url(url, app)
}

#[tauri::command]
fn save_key(api_key: String, app: AppHandle) -> Result<String, String> {
    enable_provider(api_key.trim())?;
    save_app_key(api_key.trim())?;
    let message = restart_codex_if_running().unwrap_or_else(|| "Ключ сохранен.".to_string());
    let _ = app.emit("zenith-state-changed", ());
    Ok(message)
}

#[tauri::command]
fn reset_key(app: AppHandle) -> Result<String, String> {
    reset_provider()?;
    let _ = app.emit("zenith-state-changed", ());
    Ok("Настройки восстановлены.".to_string())
}

#[tauri::command]
fn launch_saved_codex(app: AppHandle) -> Result<String, String> {
    let _ = ensure_provider_on_launch();
    if !provider_has_token() {
        return Err("Сначала сохраните API key.".to_string());
    }
    let message = launch_codex();
    close_main_window(&app);
    let _ = app.emit("zenith-state-changed", ());
    Ok(message)
}

#[tauri::command]
fn open_top_up_url(url: String, app: AppHandle) -> Result<(), String> {
    if !is_allowed_top_up_url(&url) {
        return Err("Unsupported top-up URL.".to_string());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|err| err.to_string())
}

fn is_allowed_top_up_url(value: &str) -> bool {
    let Ok(input) = Url::parse(value) else {
        return false;
    };

    if input.scheme() != "tg" || input.host_str() != Some("resolve") {
        return false;
    }

    let mut has_start = false;
    let mut has_domain = false;
    for (key, value) in input.query_pairs() {
        if key == "domain" && value == TOP_UP_BOT_DOMAIN {
            has_domain = true;
        }
        if key == "start" && !value.is_empty() {
            has_start = true;
        }
    }
    has_domain && has_start && input.fragment().is_none()
}

fn api_url(path: &str) -> String {
    format!("{}{}", DEFAULT_API_BASE_URL.trim_end_matches('/'), path)
}

async fn api_get(
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

fn normalize_api_key(api_key: &str) -> Result<String, String> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err("API key is required.".to_string());
    }
    Ok(api_key.to_string())
}

async fn api_error_message(response: reqwest::Response, fallback: &str) -> String {
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

fn sanitize_api_error_message(message: &str, fallback: &str) -> String {
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

fn contains_only_safe_public_support_links(message: &str) -> bool {
    for word in message.split_whitespace() {
        let candidate = word.trim_matches(|character: char| {
            matches!(character, '.' | ',' | ';' | ':' | ')' | ']' | '}')
        });
        if candidate.starts_with("http://")
            || candidate.starts_with("https://")
            || candidate.starts_with("tg://")
        {
            if !is_safe_public_support_link(candidate) {
                return false;
            }
        }
    }
    true
}

fn is_safe_public_support_link(value: &str) -> bool {
    let Ok(url) = Url::parse(value) else {
        return false;
    };
    if url.scheme() == "https" && url.host_str() == Some("t.me") {
        return url.path() == "/zenith_service_bot";
    }
    if url.scheme() == "tg" && url.host_str() == Some("resolve") {
        return url
            .query_pairs()
            .any(|(key, value)| key == "domain" && value == TOP_UP_BOT_DOMAIN);
    }
    false
}

fn key_stats_from_value(data: &Value) -> KeyStats {
    let enabled = data
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    KeyStats {
        masked_key: string_field(data, &["maskedKey", "masked_key", "key"]),
        label: data
            .get("label")
            .and_then(Value::as_str)
            .map(str::to_string),
        enabled,
        status: string_field(data, &["status"]),
        balance_cents: int_field(data, &["balanceCents"]),
        balance_microusd: optional_int_field(data, &["balanceMicrousd"]),
        spent_cents: int_field(data, &["spentCents"]),
        spent_microusd: optional_int_field(data, &["spentMicrousd"]),
        total_credits_cents: int_field(data, &["totalCreditsCents"]),
        total_credits_microusd: optional_int_field(data, &["totalCreditsMicrousd"]),
        requests: int_field(data, &["requests"]),
        input_tokens: int_field(data, &["inputTokens"]),
        cached_input_tokens: int_field(data, &["cachedInputTokens"]),
        reasoning_tokens: int_field(data, &["reasoningTokens"]),
        output_tokens: int_field(data, &["outputTokens"]),
        total_tokens: int_field(data, &["totalTokens"]),
        daily_spent_cents: int_field(data, &["dailySpentCents"]),
        daily_spent_microusd: optional_int_field(data, &["dailySpentMicrousd"]),
        weekly_spent_cents: int_field(data, &["weeklySpentCents"]),
        weekly_spent_microusd: optional_int_field(data, &["weeklySpentMicrousd"]),
        monthly_spent_cents: int_field(data, &["monthlySpentCents"]),
        monthly_spent_microusd: optional_int_field(data, &["monthlySpentMicrousd"]),
        balance: string_field(data, &["balance"]),
        spent: string_field(data, &["spent"]),
        total_credits: string_field(data, &["totalCredits"]),
        requests_display: string_field(data, &["requestsDisplay"]),
        input_tokens_display: string_field(data, &["inputTokensDisplay"]),
        cached_input_tokens_display: string_field(data, &["cachedInputTokensDisplay"]),
        reasoning_tokens_display: string_field(data, &["reasoningTokensDisplay"]),
        output_tokens_display: string_field(data, &["outputTokensDisplay"]),
        total_tokens_display: string_field(data, &["totalTokensDisplay"]),
        daily_spent: string_field(data, &["dailySpent"]),
        weekly_spent: string_field(data, &["weeklySpent"]),
        monthly_spent: string_field(data, &["monthlySpent"]),
    }
}

fn enrich_key_stats(stats: &mut KeyStats) {
    if stats.status.is_empty() {
        stats.status = if stats.enabled { "active" } else { "disabled" }.to_string();
    }
    if stats.balance.is_empty() {
        stats.balance = stats
            .balance_microusd
            .map(format_money_microusd)
            .unwrap_or_else(|| format_money(stats.balance_cents));
    }
    if stats.spent.is_empty() {
        stats.spent = stats
            .spent_microusd
            .map(format_money_microusd)
            .unwrap_or_else(|| format_money(stats.spent_cents));
    }
    if stats.total_credits.is_empty() {
        stats.total_credits = stats
            .total_credits_microusd
            .map(format_money_microusd)
            .unwrap_or_else(|| format_money(stats.total_credits_cents));
    }
    if stats.requests_display.is_empty() {
        stats.requests_display = format_number(stats.requests);
    }
    if stats.input_tokens_display.is_empty() {
        stats.input_tokens_display = format_number(stats.input_tokens);
    }
    if stats.cached_input_tokens_display.is_empty() {
        stats.cached_input_tokens_display = format_number(stats.cached_input_tokens);
    }
    if stats.reasoning_tokens_display.is_empty() {
        stats.reasoning_tokens_display = format_number(stats.reasoning_tokens);
    }
    if stats.output_tokens_display.is_empty() {
        stats.output_tokens_display = format_number(stats.output_tokens);
    }
    if stats.total_tokens_display.is_empty() {
        stats.total_tokens_display = format_number(stats.total_tokens);
    }
    if stats.daily_spent.is_empty() {
        stats.daily_spent = stats
            .daily_spent_microusd
            .map(format_money_microusd)
            .unwrap_or_else(|| format_money(stats.daily_spent_cents));
    }
    if stats.weekly_spent.is_empty() {
        stats.weekly_spent = stats
            .weekly_spent_microusd
            .map(format_money_microusd)
            .unwrap_or_else(|| format_money(stats.weekly_spent_cents));
    }
    if stats.monthly_spent.is_empty() {
        stats.monthly_spent = stats
            .monthly_spent_microusd
            .map(format_money_microusd)
            .unwrap_or_else(|| format_money(stats.monthly_spent_cents));
    }
}

fn enrich_usage_log_entry(entry: &mut UsageLogEntry) {
    entry.model_display = entry
        .model
        .clone()
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| "Unknown model".to_string());
    entry.created_at_display = format_api_date(&entry.created_at);
    entry.cost = entry
        .cost_microusd
        .map(format_money_microusd)
        .unwrap_or_else(|| format_money(entry.cost_cents));
    entry.input_tokens_display = format_number(entry.input_tokens);
    entry.cached_input_tokens_display = format_number(entry.cached_input_tokens);
    entry.reasoning_tokens_display = format_number(entry.reasoning_tokens);
    entry.output_tokens_display = format_number(entry.output_tokens);
    entry.total_tokens_display = format_number(entry.total_tokens);
}

fn format_money(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.abs();
    format!("{sign}${}.{:02}", format_number(abs / 100), abs % 100)
}

fn format_money_microusd(microusd: i64) -> String {
    let sign = if microusd < 0 { "-" } else { "" };
    let abs = microusd.abs();
    format!(
        "{sign}${}.{:06}",
        format_number(abs / 1_000_000),
        abs % 1_000_000
    )
}

fn format_number(value: i64) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let digits = value.abs().to_string();
    let mut output = String::new();
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            output.push(' ');
        }
        output.push(ch);
    }
    format!("{sign}{}", output.chars().rev().collect::<String>())
}

fn format_api_date(value: &str) -> String {
    match DateTime::parse_from_rfc3339(value) {
        Ok(date) => date
            .with_timezone(&Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => fallback_api_date(value),
    }
}

fn fallback_api_date(value: &str) -> String {
    let compact = value.replace('T', " ").replace('Z', "");
    let compact = compact.split('.').next().unwrap_or(value);
    compact.chars().take(16).collect()
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

fn validate_top_up_amount_cents(amount_cents: i64) -> Result<(), String> {
    if amount_cents <= 0 {
        return Err("Top-up amount must be positive.".to_string());
    }
    if amount_cents > MAX_TOP_UP_AMOUNT_CENTS {
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

fn int_field(data: &Value, keys: &[&str]) -> i64 {
    keys.iter()
        .find_map(|key| data.get(key).and_then(Value::as_i64))
        .unwrap_or_default()
}

fn optional_int_field(data: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| data.get(key).and_then(Value::as_i64))
}

fn string_field(data: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| data.get(key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_string()
}

fn extract_top_up_start(data: TopUpIntentData) -> Option<String> {
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

fn extract_top_up_start_from_url(value: &str) -> Option<String> {
    let input = Url::parse(value).ok()?;
    if input.scheme() == "tg"
        && input.host_str() == Some("resolve")
        && input
            .query_pairs()
            .any(|(key, value)| key == "domain" && value == TOP_UP_BOT_DOMAIN)
    {
        return input
            .query_pairs()
            .find_map(|(key, value)| (key == "start").then(|| value.to_string()))
            .filter(|start| is_valid_top_up_start(start));
    }

    let base = Url::parse(TOP_UP_BOT_URL).ok()?;
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

fn telegram_start_url(start: &str) -> String {
    let mut url = Url::parse("tg://resolve").expect("static tg URL is valid");
    url.query_pairs_mut()
        .append_pair("domain", TOP_UP_BOT_DOMAIN)
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

#[cfg(test)]
mod tests {
    use super::{
        extract_top_up_start, extract_top_up_start_from_url, fallback_api_date, format_api_date,
        format_money_microusd, is_allowed_top_up_url, key_stats_from_value,
        sanitize_api_error_message, telegram_start_url, validate_top_up_amount_cents,
        TopUpIntentData, MAX_TOP_UP_AMOUNT_CENTS,
    };
    use serde_json::json;

    #[test]
    fn top_up_opener_allows_only_telegram_app_deep_link() {
        assert!(is_allowed_top_up_url(
            "tg://resolve?domain=zenith_service_bot&start=ztu_0123456789abcdef0123456789abcdef0123"
        ));
        assert!(!is_allowed_top_up_url(
            "https://t.me/zenith_service_bot?start=ztu_0123456789abcdef0123456789abcdef0123"
        ));
        assert!(!is_allowed_top_up_url(
            "tg://resolve?domain=other_bot&start=ztu_0123456789abcdef0123456789abcdef0123"
        ));
    }

    #[test]
    fn top_up_start_payload_is_converted_to_app_deep_link() {
        assert_eq!(
            extract_top_up_start_from_url(
                "https://t.me/zenith_service_bot?start=ztu_0123456789abcdef0123456789abcdef0123"
            )
            .as_deref(),
            Some("ztu_0123456789abcdef0123456789abcdef0123")
        );
        assert_eq!(
            telegram_start_url("ztu_0123456789abcdef0123456789abcdef0123"),
            "tg://resolve?domain=zenith_service_bot&start=ztu_0123456789abcdef0123456789abcdef0123"
        );
    }

    #[test]
    fn top_up_start_payload_rejects_malformed_backend_values() {
        assert!(extract_top_up_start(TopUpIntentData {
            code: Some("ztu_0123456789abcdef0123456789abcdef0123".to_string()),
            start_parameter: None,
            start_payload: None,
            bot_url: None,
            url: None,
        })
        .is_some());
        assert!(extract_top_up_start(TopUpIntentData {
            code: Some("ztu_short".to_string()),
            start_parameter: None,
            start_payload: None,
            bot_url: None,
            url: None,
        })
        .is_none());
        assert_eq!(
            extract_top_up_start(TopUpIntentData {
                code: Some("ztu_0123456789abcdef0123456789abcdef0123".to_string()),
                start_parameter: Some("ztu_short".to_string()),
                start_payload: None,
                bot_url: None,
                url: None,
            })
            .as_deref(),
            Some("ztu_0123456789abcdef0123456789abcdef0123")
        );
        assert!(extract_top_up_start(TopUpIntentData {
            code: None,
            start_parameter: Some("ztu_0123456789ABCDEF0123456789ABCDEF0123".to_string()),
            start_payload: None,
            bot_url: None,
            url: None,
        })
        .is_none());
        assert!(extract_top_up_start_from_url(
            "https://t.me/zenith_service_bot?start=ztu_0123456789abcdef0123456789abcdef012g"
        )
        .is_none());
    }

    #[test]
    fn usage_history_date_uses_local_timezone_format() {
        let display = format_api_date("2026-06-05T12:34:56.789Z");

        assert_eq!(display.len(), 16);
        assert!(display.starts_with("2026-06-05 "));
        assert_ne!(display, "2026-06-05T12:34");
    }

    #[test]
    fn invalid_usage_history_date_falls_back_to_compact_display() {
        assert_eq!(
            fallback_api_date("2026-06-05T12:34:56.789Z"),
            "2026-06-05 12:34"
        );
    }

    #[test]
    fn usage_history_cost_uses_micro_usd_precision() {
        assert_eq!(format_money_microusd(77_592), "$0.077592");
        assert_eq!(format_money_microusd(1_234_567), "$1.234567");
    }

    #[test]
    fn key_stats_uses_micro_usd_precision() {
        let mut stats = key_stats_from_value(&json!({
            "maskedKey": "znt_aa...bbbb",
            "enabled": true,
            "balanceCents": 9969,
            "balanceMicrousd": 99701145,
            "spentCents": 31,
            "spentMicrousd": 298855,
            "totalCreditsCents": 10000,
            "totalCreditsMicrousd": 100000000,
            "monthlySpentCents": 31,
            "monthlySpentMicrousd": 298855
        }));

        super::enrich_key_stats(&mut stats);

        assert_eq!(stats.balance, "$99.701145");
        assert_eq!(stats.spent, "$0.298855");
        assert_eq!(stats.total_credits, "$100.000000");
        assert_eq!(stats.monthly_spent, "$0.298855");
    }

    #[test]
    fn top_up_amount_validation_rejects_invalid_ipc_amounts() {
        assert!(validate_top_up_amount_cents(100).is_ok());
        assert!(validate_top_up_amount_cents(MAX_TOP_UP_AMOUNT_CENTS).is_ok());
        assert!(validate_top_up_amount_cents(0).is_err());
        assert!(validate_top_up_amount_cents(MAX_TOP_UP_AMOUNT_CENTS + 1).is_err());
    }

    #[test]
    fn api_error_sanitizer_hides_backend_and_token_details() {
        assert_eq!(
            sanitize_api_error_message(
                "provider failed at https://upstream.example/v1 with token sk-secret and cf-ray abc",
                "Stats request failed."
            ),
            "Stats request failed."
        );
        assert_eq!(
            sanitize_api_error_message("Requested model is disabled", "Stats request failed."),
            "Requested model is disabled"
        );
        assert_eq!(
            sanitize_api_error_message(
                "Insufficient Zenith balance. Top up your Zenith API balance in the bot: https://t.me/zenith_service_bot",
                "Stats request failed."
            ),
            "Insufficient Zenith balance. Top up your Zenith API balance in the bot: https://t.me/zenith_service_bot"
        );
        assert_eq!(
            sanitize_api_error_message(
                "upstream token failed; contact https://t.me/zenith_service_bot",
                "Stats request failed."
            ),
            "Stats request failed."
        );
        assert_eq!(
            sanitize_api_error_message(
                "Insufficient Zenith balance. Top up at https://evil.example",
                "Stats request failed."
            ),
            "Stats request failed."
        );
    }
}

fn main() {
    let _single_instance = match TcpListener::bind(SINGLE_INSTANCE_ADDR) {
        Ok(listener) => listener,
        Err(_) => {
            if let Ok(mut stream) = TcpStream::connect(SINGLE_INSTANCE_ADDR) {
                let _ = stream.write_all(b"show");
            }
            return;
        }
    };

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(AppState::new())
        .setup(|app| {
            let handle = app.handle().clone();
            let _ = ensure_provider_on_launch();
            let state = app.state::<AppState>();
            build_tray(&handle, &state)?;
            start_key_stats_watcher(handle.clone());

            let instance_handle = handle.clone();
            thread::spawn(move || {
                for stream in _single_instance.incoming() {
                    if stream.is_ok() {
                        crate::tray::show_main_window(&instance_handle);
                    }
                }
            });

            if env::args().any(|arg| arg == "--tray") {
                close_main_window(&handle);
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.destroy();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            get_platform,
            get_system_locale,
            get_key_stats,
            get_key_usage_history,
            get_key_usage_version,
            create_top_up_intent_and_open,
            prepare_top_up_amount,
            save_key,
            reset_key,
            launch_saved_codex,
            open_top_up_url
        ])
        .build(tauri::generate_context!())
        .expect("failed to build Zenith Codex");

    app.run(|app_handle, event| {
        if let RunEvent::ExitRequested { api, code, .. } = event {
            let state = app_handle.state::<AppState>();
            if code.is_none() && state.should_prevent_exit() {
                api.prevent_exit();
            }
        }
    });
}

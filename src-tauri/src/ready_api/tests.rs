use super::{
    client::sanitize_api_error_message,
    commands::{api_key_page_url, parse_model_ids},
    models::{TopUpIntentData, UiState},
    top_up::{
        extract_top_up_start, extract_top_up_start_from_url, is_allowed_top_up_url,
        telegram_start_url, validate_top_up_amount_cents, MAX_AMOUNT_CENTS,
    },
};

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

#[test]
fn ui_state_never_serializes_the_saved_api_key() {
    let value = serde_json::to_value(UiState {
        provider_active: true,
        codex_running: false,
        has_saved_api_key: true,
    })
    .unwrap();
    let rendered = value.to_string();
    assert_eq!(value["hasSavedApiKey"], true);
    assert!(!rendered.contains("savedApiKey"));
    assert!(!rendered.contains("api_key"));
}

#[test]
fn model_catalog_returns_only_bounded_single_line_ids() {
    let models = parse_model_ids(
        br#"{"data":[{"id":"gpt-test"},{"id":"gpt test"},{"id":"gpt-test"},{"id":"bad\nmodel"}]}"#,
    )
    .unwrap();
    assert_eq!(models, ["gpt-test"]);
    assert!(parse_model_ids(br#"{"data":[]}"#).is_err());
    assert!(parse_model_ids(b"not-json").is_err());
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
fn top_up_amount_validation_rejects_invalid_ipc_amounts() {
    assert!(validate_top_up_amount_cents(100).is_ok());
    assert!(validate_top_up_amount_cents(MAX_AMOUNT_CENTS).is_ok());
    assert!(validate_top_up_amount_cents(0).is_err());
    assert!(validate_top_up_amount_cents(MAX_AMOUNT_CENTS + 1).is_err());
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

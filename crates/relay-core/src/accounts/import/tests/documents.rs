use super::*;

#[test]
fn rejects_duplicate_json_fields_without_exposing_secrets() {
    for input in [
        r#"{"access_token":"at-first-secret","access_token":"at-second-secret"}"#,
        r#"{"access_token":"at-first-secret","access_\u0074oken":"at-second-secret"}"#,
        r#"{"tokens":{"access_token":"at-first-secret","access_token":"at-second-secret"}}"#,
        "[\n{\"access_token\":\"at-first-secret\"},\n{\"access_token\":\"at-first-secret\",\"access_token\":\"at-second-secret\"}\n]",
        r#"{"private-secret-key":1,"private-secret-key":2}"#,
    ] {
        let error = parse_import(input, None, &[]).unwrap_err();
        assert_eq!(error.code, ImportErrorCode::MalformedJson);
        assert_eq!(error.message, "import JSON contains duplicate fields");
        assert!(!format!("{error:?}").contains("secret"));
    }
}

#[test]
fn json_lines_reject_duplicate_fields_without_discarding_other_accounts() {
    let parsed = parse_import(
        "{\"access_token\":\"at-valid\"}\n{\"access_token\":\"header.payload.signature\",\"access_\\u0074oken\":\"at-second\"}",
        None,
        &[],
    )
    .unwrap();
    assert_eq!(parsed.items.len(), 1);
    assert_eq!(parsed.items[0].secrets().access_token(), Some("at-valid"));
    assert_eq!(parsed.preview.rows.len(), 2);
    assert_eq!(parsed.preview.rows[1].status, ImportPreviewStatus::Invalid);
    assert_eq!(
        parsed.preview.rows[1].error.as_ref().unwrap().code,
        ImportIssueCode::MalformedJson
    );
}

#[test]
fn duplicate_first_json_line_does_not_hide_the_next_valid_account() {
    let parsed = parse_import("{\"access_token\":\"at-first\",\"access_token\":\"at-second\"}\n{\"access_token\":\"at-valid\"}", None, &[]).unwrap();
    assert_eq!(parsed.items.len(), 1);
    assert_eq!(parsed.items[0].secrets().access_token(), Some("at-valid"));
    assert_eq!(parsed.preview.rows[0].status, ImportPreviewStatus::Invalid);
}

#[test]
fn strict_json_validation_preserves_number_precision_and_independent_object_keys() {
    let parsed = parse_import(r#"[{"access_token":"at-one","metadata":{"value":1.2345678901234567890123456789}},{"access_token":"at-two","metadata":{"value":123456789012345678901234567890}}]"#, None, &[]).unwrap();
    assert_eq!(parsed.items.len(), 2);
}

#[test]
fn import_auth_mode_names_are_stable() {
    assert_eq!(ImportAuthMode::OAuth.as_str(), "oauth");
    assert_eq!(ImportAuthMode::AgentIdentity.as_str(), "agent_identity");
    assert_eq!(ImportAuthMode::ApiKey.as_str(), "api_key");
    assert_eq!(ImportAuthMode::ImportedToken.as_str(), "imported_token");
    assert_eq!(ImportAuthMode::Unknown.as_str(), "unknown");
}

#[test]
fn combines_multiple_files_and_nested_account_containers() {
    let documents = vec![
        format!(
            r#"{{"account_id":"account-one","email":"one@example.test","access_token":"{ACCESS}"}}"#
        ),
        format!(
            r#"[{{"account_id":"account-two","email":"two@example.test","access_token":"{ACCESS}-two"}},{{"account_id":"account-three","email":"three@example.test","access_token":"{ACCESS}-three"}}]"#
        ),
        format!(
            r#"{{"accounts":[{{"account_id":"account-four","email":"four@example.test","access_token":"{ACCESS}-four"}}]}}"#
        ),
    ];

    let combined = combine_import_documents(&documents).unwrap();
    let parsed = parse_import(&combined, None, &[]).unwrap();

    assert_eq!(parsed.preview.format, ImportFormat::JsonArray);
    assert_eq!(parsed.preview.rows.len(), 4);
    assert_eq!(parsed.items.len(), 4);
    assert_eq!(
        parsed
            .items
            .iter()
            .map(|item| item.identity_key.as_str())
            .collect::<HashSet<_>>()
            .len(),
        4
    );
}

#[test]
fn combines_valid_files_with_malformed_files_as_error_rows() {
    let documents = vec![
        format!(r#"{{"account_id":"account-one","access_token":"{ACCESS}"}}"#),
        r#"{"access_token":"truncated""#.to_string(),
        "Bearer header.payload.signature".to_string(),
    ];

    let combined = combine_import_documents(&documents).unwrap();
    let parsed = parse_import(&combined, None, &[]).unwrap();

    assert_eq!(parsed.preview.rows.len(), 3);
    assert_eq!(parsed.items.len(), 2);
    assert_eq!(parsed.preview.rows[1].status, ImportPreviewStatus::Invalid);
    assert_eq!(
        parsed.preview.rows[1]
            .error
            .as_ref()
            .map(|error| error.code),
        Some(ImportIssueCode::MalformedJson)
    );
}

#[test]
fn parses_raw_access_tokens_with_bearer_prefix_and_token_lines() {
    let input = "Bearer header.payload.signature\nat-opaque-token\n\"at-quoted-token\"";
    let parsed = parse_import(input, None, &[]).unwrap();

    assert_eq!(parsed.preview.format, ImportFormat::JsonLines);
    assert_eq!(parsed.items.len(), 3);
    assert_eq!(
        parsed.items[0].secrets().access_token(),
        Some("header.payload.signature")
    );
    assert_eq!(
        parsed.items[1].secrets().access_token(),
        Some("at-opaque-token")
    );
    assert_eq!(
        parsed.items[2].secrets().access_token(),
        Some("at-quoted-token")
    );
    let preview = serde_json::to_string(&parsed.preview).unwrap();
    assert!(!preview.contains("opaque-token"));

    let array = parse_import(r#"["at-array-token","Bearer one.two.three"]"#, None, &[]).unwrap();
    assert_eq!(array.items.len(), 2);
    assert_eq!(
        array.items[1].secrets().access_token(),
        Some("one.two.three")
    );
}

#[test]
fn parses_nested_account_subscription_metadata_for_opaque_tokens() {
    let parsed = parse_import(
        r#"{
            "access_token":"at-private-token",
            "account":{
                "id":"account-team",
                "email":"team@example.test",
                "planType":"team",
                "subscriptionActiveUntil":"2026-10-19T14:17:45Z"
            }
        }"#,
        None,
        &[],
    )
    .unwrap();

    assert_eq!(parsed.preview.rows[0].plan.as_deref(), Some("team"));
    assert_eq!(
        parsed.preview.rows[0].subscription_expires_at.as_deref(),
        Some("2026-10-19T14:17:45Z")
    );
    assert_eq!(parsed.items[0].account_id.as_deref(), Some("account-team"));
    assert!(!serde_json::to_string(&parsed.preview)
        .unwrap()
        .contains("at-private-token"));
}

#[test]
fn token_expiry_is_retained_for_the_credential_not_only_the_preview() {
    let parsed = parse_import(
        r#"{"access_token":"at-synthetic","expires_at":"2026-10-19T14:17:45Z"}"#,
        None,
        &[],
    )
    .unwrap();
    assert_eq!(parsed.items[0].expires_at_ms, Some(1_792_419_465_000));
    let unknown = parse_import(r#"{"access_token":"at-synthetic"}"#, None, &[]).unwrap();
    assert_eq!(unknown.items[0].expires_at_ms, None);
}

#[test]
fn id_token_expiry_never_becomes_the_access_token_expiry() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    let token = |expires| {
        format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD.encode(serde_json::json!({"exp": expires}).to_string()),
        )
    };
    for (access_token, expected) in [
        (token(2_000), Some(2_000_000)),
        ("opaque-access-token".to_string(), None),
    ] {
        let parsed = parse_import(
            &serde_json::json!({"id_token": token(1_000), "access_token": access_token})
                .to_string(),
            None,
            &[],
        )
        .unwrap();
        assert_eq!(parsed.items[0].expires_at_ms, expected);
    }
}

#[test]
fn parses_login_notes_nested_beside_the_account_email() {
    let parsed = parse_import(
        r#"{
            "access_token":"at-private-token",
            "account":{
                "email":"person@example.test",
                "phone":"950-000-000",
                "password":"synthetic-password",
                "2fa":"gezd gnbv-gy3t qojq"
            }
        }"#,
        None,
        &[],
    )
    .unwrap();
    let item = &parsed.items[0];
    assert_eq!(item.email(), Some("person@example.test"));
    assert_eq!(item.phone(), Some("950-000-000"));
    assert_eq!(item.password(), Some("synthetic-password"));
    assert_eq!(item.totp_secret(), Some("GEZDGNBVGY3TQOJQ"));
    let debug = format!("{item:?}");
    assert!(!debug.contains("synthetic-password"));
    assert!(!debug.contains("GEZDGNBVGY3TQOJQ"));
}

#[test]
fn parses_codex_auth_json_token_and_api_key_shapes() {
    let oauth = parse_import(
        &format!(
            r#"{{"auth_mode":"chatgpt","OPENAI_API_KEY":"{API_KEY}","tokens":{{"access_token":"{ACCESS}","refresh_token":"{REFRESH}","id_token":"{ID}"}},"email":"{EMAIL}"}}"#
        ),
        Some("auth.json"),
        &[],
    )
    .unwrap();
    assert_eq!(oauth.preview.rows[0].auth_mode, ImportAuthMode::OAuth);
    assert_eq!(oauth.items[0].secrets().access_token(), Some(ACCESS));
    assert_eq!(oauth.items[0].secrets().refresh_token(), Some(REFRESH));
    assert_eq!(oauth.items[0].secrets().id_token(), Some(ID));
    assert_eq!(oauth.items[0].secrets().api_key(), None);
    assert_eq!(oauth.items[0].email(), Some(EMAIL));
    assert_ne!(oauth.preview.rows[0].identity, EMAIL);
    assert!(oauth.preview.rows[0]
        .warnings
        .iter()
        .any(|warning| { warning.code == ImportWarningCode::UnusedCredentialsIgnored }));

    let api_key = parse_import(
        &format!(r#"{{"auth_mode":"apikey","OPENAI_API_KEY":"{API_KEY}"}}"#),
        None,
        &[],
    )
    .unwrap();
    assert_eq!(api_key.preview.rows[0].auth_mode, ImportAuthMode::ApiKey);
    assert_eq!(api_key.items[0].secrets().api_key(), Some(API_KEY));
}

#[test]
fn parses_top_level_nested_and_degraded_token_shapes() {
    let top_level = parse_import(
        &format!(r#"{{"access_token":"{ACCESS}","refresh_token":"{REFRESH}","id_token":"{ID}"}}"#),
        None,
        &[],
    )
    .unwrap();
    assert_eq!(
        top_level.preview.rows[0].auth_mode,
        ImportAuthMode::ImportedToken
    );

    let nested = parse_import(
        &format!(r#"{{"tokens":{{"accessToken":"{ACCESS}"}}}}"#),
        None,
        &[],
    )
    .unwrap();
    assert_eq!(nested.items[0].secrets().access_token(), Some(ACCESS));
    assert!(nested.preview.rows[0]
        .warnings
        .iter()
        .any(|warning| warning.code == ImportWarningCode::AccessTokenOnly));

    let refresh_only =
        parse_import(&format!(r#"{{"refresh_token":"{REFRESH}"}}"#), None, &[]).unwrap();
    assert_eq!(
        refresh_only.items[0].secrets().refresh_token(),
        Some(REFRESH)
    );
    assert!(refresh_only.preview.rows[0]
        .warnings
        .iter()
        .any(|warning| warning.code == ImportWarningCode::RefreshExchangeRequired));
}

#[test]
fn parses_api_key_metadata_array_and_json_lines() {
    let array = parse_import(
        &format!(
            r#"[{{"api_key":"{API_KEY}","base_url":"https://api.example.test/v1?discard=1","protocol":"responses"}},{{"access_token":"{ACCESS}"}}]"#
        ),
        None,
        &[],
    )
    .unwrap();
    assert_eq!(array.preview.format, ImportFormat::JsonArray);
    assert_eq!(array.items.len(), 2);
    assert_eq!(
        array.items[0].base_url.as_deref(),
        Some("https://api.example.test/v1")
    );
    assert_eq!(array.items[0].protocol.as_deref(), Some("responses"));

    let json_lines = parse_import(
        &format!(
            "{{\"access_token\":\"{ACCESS}\"}}\nnot-json-{API_KEY}\n{{\"refresh_token\":\"{REFRESH}\"}}"
        ),
        None,
        &[],
    )
    .unwrap();
    assert_eq!(json_lines.preview.format, ImportFormat::JsonLines);
    assert_eq!(json_lines.preview.rows.len(), 3);
    assert_eq!(json_lines.items.len(), 2);
    assert_eq!(
        json_lines.preview.rows[1].status,
        ImportPreviewStatus::Invalid
    );
}

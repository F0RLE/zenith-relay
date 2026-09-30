use super::*;

#[test]
fn duplicates_and_existing_items_have_safe_selection_states() {
    let input = format!(
        r#"[{{"email":"{EMAIL}","access_token":"{ACCESS}"}},{{"email":"{EMAIL}","access_token":"different-secret"}}]"#
    );
    let first = parse_import(&input, None, &[]).unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.preview.rows[1].status, ImportPreviewStatus::Invalid);
    assert_eq!(
        first.preview.rows[1].error.as_ref().map(|error| error.code),
        Some(ImportIssueCode::DuplicateItem)
    );

    let existing_key = first.items[0].identity_key.clone();
    let existing = parse_import(
        &format!(r#"{{"email":"{EMAIL}","access_token":"{ACCESS}"}}"#),
        None,
        &[existing_key],
    )
    .unwrap();
    assert_eq!(
        existing.preview.rows[0].status,
        ImportPreviewStatus::Existing
    );
    assert!(existing.preview.rows[0].selectable);
    assert!(!existing.preview.rows[0].default_selected);
    assert!(existing.preview.rows[0].existing);

    let different_kinds = parse_import(
        &format!(
            r#"[{{"auth_mode":"apikey","email":"{EMAIL}","OPENAI_API_KEY":"{API_KEY}"}},{{"auth_mode":"chatgpt","email":"{EMAIL}","access_token":"{ACCESS}"}}]"#
        ),
        None,
        &[],
    )
    .unwrap();
    assert_eq!(different_kinds.items.len(), 2);
    assert_ne!(
        different_kinds.preview.rows[0].item_id,
        different_kinds.preview.rows[1].item_id
    );
}

#[test]
fn shared_team_account_id_does_not_merge_distinct_users() {
    let input = format!(
        r#"[{{"account_id":"shared-team","email":"one@example.test","access_token":"{ACCESS}"}},{{"account_id":"shared-team","email":"two@example.test","access_token":"different-secret"}}]"#
    );
    let parsed = parse_import(&input, None, &[]).unwrap();
    assert_eq!(parsed.items.len(), 2);
    assert_ne!(parsed.items[0].identity_key, parsed.items[1].identity_key);
    assert!(parsed.preview.rows.iter().all(|row| row.selectable));
}

#[test]
fn limits_and_malformed_input_return_redacted_errors() {
    let oversized = "x".repeat(MAX_IMPORT_BYTES + 1);
    assert_eq!(
        parse_import(&oversized, None, &[]).unwrap_err().code,
        ImportErrorCode::InputTooLarge
    );

    let too_many = format!(
        "[{}]",
        std::iter::repeat_n(
            format!(r#"{{"access_token":"{ACCESS}"}}"#),
            MAX_IMPORT_ITEMS + 1
        )
        .collect::<Vec<_>>()
        .join(",")
    );
    assert_eq!(
        parse_import(&too_many, None, &[]).unwrap_err().code,
        ImportErrorCode::TooManyItems
    );

    let mut deep = format!(r#"{{"access_token":"{ACCESS}","deep":"#);
    deep.push_str(&"[".repeat(MAX_JSON_DEPTH + 1));
    deep.push_str("null");
    deep.push_str(&"]".repeat(MAX_JSON_DEPTH + 1));
    deep.push('}');
    assert_eq!(
        parse_import(&deep, None, &[]).unwrap_err().code,
        ImportErrorCode::JsonTooDeep
    );

    let malformed = format!(r#"{{"access_token":"{ACCESS}""#);
    let error = parse_import(&malformed, None, &[]).unwrap_err();
    let serialized = serde_json::to_string(&error).unwrap();
    assert_eq!(error.code, ImportErrorCode::MalformedJson);
    assert!(!serialized.contains(ACCESS));
}

#[test]
fn previews_errors_and_debug_output_never_contain_fixture_secrets() {
    let input = format!(
        r#"{{"name":"{ACCESS}","email":"{EMAIL}","access_token":"{ACCESS}","refresh_token":"{REFRESH}","id_token":"{ID}","plan_type":"{ACCESS}","expires_at":"{REFRESH}"}}"#
    );
    let parsed = parse_import(&input, Some("access-super-secret.json"), &[]).unwrap();
    let preview = serde_json::to_string(&parsed.preview).unwrap();
    let debug = format!("{parsed:?} {:?}", parsed.items[0].secrets());
    for secret in [ACCESS, REFRESH, ID, API_KEY, EMAIL] {
        assert!(!preview.contains(secret));
        assert!(!debug.contains(secret));
    }
    assert!(debug.contains("[redacted]"));

    let invalid = parse_import(
        &format!(r#"{{"api_key":"{API_KEY}","access_token":"{ACCESS}"}}"#),
        None,
        &[],
    )
    .unwrap();
    let invalid_preview = serde_json::to_string(&invalid.preview).unwrap();
    assert!(!invalid_preview.contains(API_KEY));
    assert!(!invalid_preview.contains(ACCESS));
}

#[test]
fn unsafe_source_names_and_unsupported_bundle_versions_are_rejected() {
    let input = format!(r#"{{"access_token":"{ACCESS}"}}"#);
    assert_eq!(
        parse_import(&input, Some("../auth.json"), &[])
            .unwrap_err()
            .code,
        ImportErrorCode::InvalidSourceFile
    );
    assert_eq!(
        parse_import(
            r#"{"type":"portable_account_bundle","version":2,"accounts":[]}"#,
            None,
            &[],
        )
        .unwrap_err()
        .code,
        ImportErrorCode::UnsupportedBundleVersion
    );
}

fn unsigned_jwt(payload: &str) -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    format!(
        "{}.{}.signature",
        URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#),
        URL_SAFE_NO_PAD.encode(payload.as_bytes())
    )
}

#[test]
fn parses_current_session_header_and_wrapped_sub2api_exports() {
    let session = parse_import(
        r#"{
            "user":{"id":"user_session","email":"session@example.test"},
            "account":{"id":"acct_session","planType":"plus"},
            "accessToken":"at-session-token",
            "sessionToken":"web-session-secret",
            "expires":"2026-10-01T00:00:00Z"
        }"#,
        None,
        &[],
    )
    .unwrap();
    assert_eq!(session.items[0].email(), Some("session@example.test"));
    assert_eq!(
        session.items[0].chatgpt_user_id.as_deref(),
        Some("user_session")
    );
    assert_eq!(session.items[0].account_id.as_deref(), Some("acct_session"));
    assert_eq!(session.preview.rows[0].plan.as_deref(), Some("plus"));
    assert_eq!(session.items[0].secrets().refresh_token(), None);
    assert!(session.preview.rows[0]
        .warnings
        .iter()
        .any(|warning| warning.code == ImportWarningCode::AccessTokenOnly));
    assert!(!serde_json::to_string(&session.preview)
        .unwrap()
        .contains("web-session-secret"));

    let cpa_session = parse_import(
        r#"{
            "type":"codex",
            "access_token":"at-cpa-token",
            "refresh_token":"",
            "session_token":"web-session-secret",
            "account_id":"acct_cpa_session"
        }"#,
        None,
        &[],
    )
    .unwrap();
    assert_eq!(cpa_session.items[0].secrets().refresh_token(), None);
    assert_eq!(
        cpa_session.items[0].account_id.as_deref(),
        Some("acct_cpa_session")
    );

    let header = parse_import(
        r#"{
            "access_token":"at-header-token",
            "account_email":"header@example.test",
            "customHeaders":{"Chatgpt-Account-Id":"acct_header"}
        }"#,
        None,
        &[],
    )
    .unwrap();
    assert_eq!(header.items[0].email(), Some("header@example.test"));
    assert_eq!(header.items[0].account_id.as_deref(), Some("acct_header"));

    let workspace = parse_import(
        r#"{"personal_access_token":"at-workspace-token","workspaceId":"acct_workspace"}"#,
        None,
        &[],
    )
    .unwrap();
    assert_eq!(
        workspace.items[0].account_id.as_deref(),
        Some("acct_workspace")
    );

    let token = unsigned_jwt(
        r#"{"email":"jwt.user@example.test","https://api.openai.com/auth":{"chatgpt_account_id":"acct_jwt","chatgpt_user_id":"user_jwt","chatgpt_plan_type":"pro"}}"#,
    );
    let jwt = parse_import(&format!(r#"{{"access_token":"{token}"}}"#), None, &[]).unwrap();
    assert_eq!(jwt.items[0].email(), Some("jwt.user@example.test"));
    assert_eq!(jwt.items[0].account_id.as_deref(), Some("acct_jwt"));
    assert_eq!(jwt.items[0].chatgpt_user_id.as_deref(), Some("user_jwt"));
    assert_eq!(jwt.preview.rows[0].plan.as_deref(), Some("pro"));

    let wrapped = parse_import(
        r#"{
            "code":0,
            "data":{
                "type":"sub2api-bundle",
                "version":1,
                "proxies":[{"name":"ignored"}],
                "accounts":[{
                    "name":"Wrapped",
                    "platform":"openai",
                    "type":"oauth",
                    "credentials":{
                        "access_token":"at-wrapped-token",
                        "refresh_token":"refresh-wrapped",
                        "email":"wrapped@example.test",
                        "chatgpt_account_id":"acct_wrapped"
                    }
                }]
            }
        }"#,
        None,
        &[],
    )
    .unwrap();
    assert_eq!(
        wrapped.preview.format,
        ImportFormat::PortableAccountBundleV1
    );
    assert_eq!(wrapped.items.len(), 1);
    assert_eq!(wrapped.items[0].email(), Some("wrapped@example.test"));
    assert_eq!(
        wrapped.items[0].secrets().refresh_token(),
        Some("refresh-wrapped")
    );
    assert!(wrapped
        .preview
        .warnings
        .iter()
        .any(|warning| warning.code == ImportWarningCode::ProxiesIgnored));
    assert!(!serde_json::to_string(&wrapped.preview)
        .unwrap()
        .contains("refresh-wrapped"));
}

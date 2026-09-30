use super::*;

#[test]
fn portable_bundle_is_neutral_and_never_imports_proxies() {
    let input = format!(
        r#"{{
            "type":"sb2api",
            "version":1,
            "exported_at":"2026-07-10T00:00:00Z",
            "proxies":[{{"url":"http://proxy-secret.test"}},{{"url":"http://other.test"}}],
            "accounts":[{{
                "name":"Personal account",
                "type":"chatgpt",
                "platform":"openai",
                "priority":7,
                "concurrency":3,
                "credentials":{{
                    "access_token":"{ACCESS}",
                    "refresh_token":"{REFRESH}",
                    "id_token":"{ID}",
                    "expires_at":1783692000,
                    "email":"{EMAIL}",
                    "chatgpt_account_id":"acct_1234567890",
                    "chatgpt_user_id":"user_1234567890",
                    "organization_id":"org_1234567890",
                    "plan_type":"plus",
                    "subscription_expires_at":"2026-08-10T00:00:00Z"
                }}
            }}]
        }}"#
    );
    let parsed = parse_import(&input, Some("portable.json"), &[]).unwrap();
    assert_eq!(parsed.preview.format, ImportFormat::PortableAccountBundleV1);
    assert_eq!(parsed.items.len(), 1);
    assert_eq!(parsed.items[0].priority, Some(7));
    assert_eq!(parsed.preview.warnings.len(), 1);
    assert_eq!(
        parsed.preview.warnings[0],
        ImportWarning::count(ImportWarningCode::ProxiesIgnored, 2)
    );
    assert!(parsed.preview.rows[0]
        .warnings
        .iter()
        .any(|warning| warning.code == ImportWarningCode::ConcurrencyIgnored));
    let preview = serde_json::to_string(&parsed.preview).unwrap();
    assert!(!preview.to_ascii_lowercase().contains("sb2api"));
    assert!(!preview.contains("proxy-secret"));
}

#[test]
fn zenith_bundle_preserves_description_and_nested_account_data() {
    let input = format!(
        r#"{{
            "format":"zenith",
            "version":1,
            "exportedAt":"2026-07-19T00:00:00Z",
            "description":"Seller description",
            "accounts":[{{
                "name":"Business account",
                "provider":"openai",
                "auth":{{
                    "type":"oauth",
                    "accessToken":"{ACCESS}",
                    "refreshToken":"{REFRESH}",
                    "idToken":"{ID}",
                    "expiresAt":"2026-08-19T00:00:00Z"
                }},
                "identity":{{
                    "email":"{EMAIL}",
                    "accountId":"acct_zenith",
                    "userId":"user_zenith",
                    "organizationId":"org_zenith"
                }},
                "subscription":{{
                    "plan":"business",
                    "expiresAt":"2026-09-19T00:00:00Z"
                }}
            }}]
        }}"#
    );
    let parsed = parse_import(&input, Some("zenith-accounts.json"), &[]).unwrap();

    assert_eq!(parsed.preview.format, ImportFormat::ZenithV1);
    assert_eq!(
        parsed.preview.description.as_deref(),
        Some("Seller description")
    );
    assert_eq!(parsed.preview.rows[0].source_name, "zenith");
    assert_eq!(parsed.preview.rows[0].plan.as_deref(), Some("business"));
    assert_eq!(
        parsed.preview.rows[0].subscription_expires_at.as_deref(),
        Some("2026-09-19T00:00:00Z")
    );
    assert_eq!(parsed.items[0].secrets().access_token(), Some(ACCESS));
    assert_eq!(parsed.items[0].secrets().refresh_token(), Some(REFRESH));
    assert_eq!(parsed.items[0].account_id.as_deref(), Some("acct_zenith"));
    assert_eq!(
        parsed.items[0].chatgpt_user_id.as_deref(),
        Some("user_zenith")
    );
    assert_eq!(
        parsed.items[0].organization_id.as_deref(),
        Some("org_zenith")
    );

    let unsupported = input.replacen("\"version\":1", "\"version\":2", 1);
    assert_eq!(
        parse_import(&unsupported, None, &[]).unwrap_err().code,
        ImportErrorCode::UnsupportedBundleVersion
    );
}

#[test]
fn parses_login_notes_and_keeps_them_out_of_debug() {
    let parsed = parse_import(
        r#"{"account_id":"acct_login","access_token":"access-super-secret","email":"person@example.test","phone":"950-000-000","password":"synthetic-password","2fa":"gezd gnbv-gy3t qojq"}"#,
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
    assert!(!debug.contains("person@example.test"));

    let dropped = parse_import(
        r#"{"account_id":"acct_login","access_token":"access-super-secret","password":"bad\nsecret","2fa":"11111111"}"#,
        None,
        &[],
    )
    .unwrap();
    assert_eq!(dropped.items[0].password(), None);
    assert_eq!(dropped.items[0].totp_secret(), None);
    assert_eq!(
        dropped.items[0].secrets().access_token(),
        Some("access-super-secret")
    );
}

#[test]
fn parses_current_public_account_export_shapes() {
    let fixtures = [
        format!(
            r#"{{"type":"codex","access_token":"{ACCESS}","refresh_token":"{REFRESH}","id_token":"{ID}","email":"{EMAIL}","account_id":"acct_cpa","plan_type":"plus","expired":"2026-08-10T00:00:00Z"}}"#
        ),
        format!(
            r#"{{"exported_at":"2026-07-10T00:00:00Z","proxies":[],"accounts":[{{"name":"sub2api account","platform":"openai","type":"oauth","credentials":{{"access_token":"{ACCESS}","email":"{EMAIL}","chatgpt_account_id":"acct_sub2api","plan_type":"plus"}}}}]}}"#
        ),
        format!(
            r#"{{"type":"codex","id_token":"{ID}","access_token":"{ACCESS}","refresh_token":"{REFRESH}","account_id":"acct_cockpit","email":"{EMAIL}","expired":"2026-08-10T00:00:00Z"}}"#
        ),
        format!(
            r#"{{"accessToken":"{ACCESS}","refreshToken":"{REFRESH}","email":"{EMAIL}","name":"9router account","authType":"oauth","providerSpecificData":{{"chatgptAccountId":"acct_9router","chatgptPlanType":"plus"}}}}"#
        ),
        format!(
            r#"{{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{{"id_token":"{ID}","access_token":"{ACCESS}","refresh_token":"{REFRESH}","account_id":"acct_codex"}},"last_refresh":"2026-07-10T00:00:00Z"}}"#
        ),
        format!(
            r#"{{"auth_mode":"chatgpt","tokens":{{"access_token":"{ACCESS}","refresh_token":"__missing_refresh_token__","id_token":"{ID}"}},"last_refresh":"2026-07-10T00:00:00Z"}}"#
        ),
        format!(
            r#"{{"tokens":{{"access_token":"{ACCESS}","refresh_token":"","id_token":"","chatgpt_account_id":"acct_manager"}},"meta":{{"label":"Manager account","workspace_id":"workspace_1","chatgpt_account_id":"acct_manager"}}}}"#
        ),
    ];

    for (index, fixture) in fixtures.iter().enumerate() {
        let parsed = parse_import(fixture, None, &[])
            .unwrap_or_else(|error| panic!("fixture {index} failed: {error}"));
        assert_eq!(parsed.items.len(), 1, "fixture {index}");
        assert_eq!(
            parsed.items[0].secrets().access_token(),
            Some(ACCESS),
            "fixture {index}"
        );
        let preview = serde_json::to_string(&parsed.preview).unwrap();
        for secret in [ACCESS, REFRESH, ID, EMAIL] {
            assert!(!preview.contains(secret), "fixture {index}");
        }
    }

    let nine_router = parse_import(&fixtures[3], None, &[]).unwrap();
    assert_eq!(nine_router.preview.format, ImportFormat::JsonObject);
    assert_eq!(
        nine_router.items[0].account_id.as_deref(),
        Some("acct_9router")
    );
    assert_eq!(nine_router.preview.rows[0].plan.as_deref(), Some("plus"));
    let sub2api = parse_import(&fixtures[1], None, &[]).unwrap();
    assert_eq!(sub2api.preview.rows[0].auth_mode, ImportAuthMode::OAuth);
    assert!(!sub2api.preview.rows[0]
        .warnings
        .iter()
        .any(|warning| warning.code == ImportWarningCode::UnknownAuthMode));
    let wrapped_nine_router =
        parse_import(&format!(r#"{{"accounts":[{}]}}"#, fixtures[3]), None, &[]).unwrap();
    assert_eq!(wrapped_nine_router.preview.format, ImportFormat::JsonArray);
    assert_eq!(wrapped_nine_router.items.len(), 1);

    let axon_hub = parse_import(&fixtures[5], None, &[]).unwrap();
    assert_eq!(axon_hub.items[0].secrets().refresh_token(), None);
    assert!(axon_hub.preview.rows[0]
        .warnings
        .iter()
        .any(|warning| warning.code == ImportWarningCode::AccessTokenOnly));

    let manager = parse_import(&fixtures[6], None, &[]).unwrap();
    assert_eq!(manager.preview.rows[0].label, "Manager account");
    assert_eq!(manager.items[0].account_id.as_deref(), Some("acct_manager"));
    for access_only in [parse_import(&fixtures[1], None, &[]).unwrap(), manager] {
        assert_eq!(access_only.items[0].secrets().refresh_token(), None);
        assert!(access_only.preview.rows[0]
            .warnings
            .iter()
            .any(|warning| warning.code == ImportWarningCode::AccessTokenOnly));
    }
}

#[test]
fn parses_mixed_cockpit_array_and_sub2api_failure_rows() {
    let cockpit = format!(
        r#"[
            {{"type":"codex","access_token":"{ACCESS}-one","refresh_token":"{REFRESH}-one","account_id":"acct_cockpit_one","email":"one@example.test"}},
            {{"type":"codex","access_token":"{ACCESS}-two","account_id":"acct_cockpit_two","email":"two@example.test"}},
            {{"auth_mode":"apikey","OPENAI_API_KEY":"{API_KEY}","api_base_url":"https://api.example.test/v1","api_provider_name":"Example API"}}
        ]"#
    );
    let parsed = parse_import(&cockpit, None, &[]).unwrap();
    assert_eq!(parsed.preview.format, ImportFormat::JsonArray);
    assert_eq!(parsed.items.len(), 3);
    assert!(parsed.preview.rows.iter().all(|row| row.selectable));
    assert_eq!(parsed.items[2].label, "Example API");
    assert_eq!(
        parsed.items[2].base_url.as_deref(),
        Some("https://api.example.test/v1")
    );

    let sub2api = format!(
        r#"{{"type":"sub2api-data","version":1,"accounts":[
            {{"name":"First","platform":"openai","type":"oauth","credentials":{{"access_token":"{ACCESS}-one","chatgpt_account_id":"acct_sub2api_one","email":"one@example.test"}}}},
            {{"name":"Second","platform":"openai","type":"oauth","credentials":{{"access_token":"{ACCESS}-two","chatgpt_account_id":"acct_sub2api_two","email":"two@example.test"}}}},
            {{"name":"Missing credential","platform":"openai","type":"oauth","credentials":{{"access_token":"","email":"missing@example.test"}}}}
        ],"proxies":[]}}"#
    );
    let parsed = parse_import(&sub2api, None, &[]).unwrap();
    assert_eq!(parsed.preview.format, ImportFormat::PortableAccountBundleV1);
    assert_eq!(parsed.preview.rows.len(), 3);
    assert_eq!(parsed.items.len(), 2);
    assert!(parsed
        .preview
        .rows
        .iter()
        .take(2)
        .all(|row| row.auth_mode == ImportAuthMode::OAuth));
    assert_eq!(parsed.preview.rows[2].status, ImportPreviewStatus::Invalid);
    assert_eq!(
        parsed.preview.rows[2]
            .error
            .as_ref()
            .map(|error| error.code),
        Some(ImportIssueCode::MissingCredentials)
    );
}

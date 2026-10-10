use super::*;
use serde_json::json;

const ACCESS: &str = "synthetic-export-access-token";
const REFRESH: &str = "synthetic-export-refresh-token";
const ID_TOKEN: &str = "synthetic.export.id-token";

#[test]
fn all_supported_formats_are_valid_json_and_never_include_proxy_fields() {
    for format in AccountExportFormat::all() {
        let document = build_account_export(format, &[fixture()], 1_788_000_000_000, None).unwrap();
        document.validate().unwrap();
        let value: Value = serde_json::from_str(&document.content).unwrap();
        assert!(document.content.contains(ACCESS), "{format:?}");
        assert!(document.content.contains(REFRESH), "{format:?}");
        if format != AccountExportFormat::NineRouter {
            assert!(document.content.contains(ID_TOKEN), "{format:?}");
        }
        assert!(!document.content.contains("proxy.example"), "{format:?}");
        assert!(!document.content.contains("proxy_url"), "{format:?}");
        assert!(value.is_object());
    }
}

#[test]
fn sub2api_matches_the_versioned_popular_export_container() {
    let document = build_account_export(
        AccountExportFormat::Sub2api,
        &[fixture(), fixture()],
        1_788_000_000_000,
        None,
    )
    .unwrap();
    let value: Value = serde_json::from_str(&document.content).unwrap();
    assert_eq!(value["type"], "sub2api-data");
    assert_eq!(value["version"], 1);
    assert_eq!(value["proxies"], json!([]));
    assert_eq!(value["accounts"].as_array().unwrap().len(), 2);
    assert_eq!(value["accounts"][0]["credentials"]["plan_type"], "plus");
    assert_eq!(document.file_name, "accounts-sub2api.json");
}

#[test]
fn cockpit_export_carries_safe_name_and_tags() {
    let document = build_account_export(
        AccountExportFormat::Cockpit,
        &[fixture()],
        1_788_000_000_000,
        None,
    )
    .unwrap();
    let value: Value = serde_json::from_str(&document.content).unwrap();

    assert_eq!(value["account_name"], "Synthetic Plus");
    assert_eq!(value["tags"], json!(["team", "work"]));

    let parsed = crate::accounts::parse_import(&document.content, None, &[]).unwrap();
    assert_eq!(
        parsed.items[0].tags,
        std::collections::BTreeSet::from(["team".to_string(), "work".to_string()])
    );
}

#[test]
fn single_exports_are_objects_and_bulk_exports_are_arrays() {
    for format in AccountExportFormat::all().into_iter().filter(|format| {
        !matches!(
            format,
            AccountExportFormat::Zenith | AccountExportFormat::Sub2api
        )
    }) {
        let single = build_account_export(format, &[fixture()], 1_788_000_000_000, None).unwrap();
        let bulk =
            build_account_export(format, &[fixture(), fixture()], 1_788_000_000_000, None).unwrap();
        assert!(serde_json::from_str::<Value>(&single.content)
            .unwrap()
            .is_object());
        assert!(serde_json::from_str::<Value>(&bulk.content)
            .unwrap()
            .is_array());
    }
}

#[test]
fn zenith_is_a_versioned_described_account_bundle() {
    let document = build_account_export(
        AccountExportFormat::Zenith,
        &[fixture(), fixture()],
        1_788_000_000_000,
        Some("  Seller description\nSecond line  "),
    )
    .unwrap();
    let value: Value = serde_json::from_str(&document.content).unwrap();

    assert_eq!(value["format"], "zenith");
    assert_eq!(value["version"], 1);
    assert_eq!(value["description"], "  Seller description\nSecond line  ");
    assert_eq!(value["accounts"].as_array().unwrap().len(), 2);
    assert_eq!(value["accounts"][0]["provider"], "openai");
    assert_eq!(value["accounts"][0]["auth"]["type"], "oauth");
    assert_eq!(
        value["accounts"][0]["identity"]["accountId"],
        "account-secret-id"
    );
    assert_eq!(value["accounts"][0]["subscription"]["plan"], "plus");
    assert!(value.get("proxies").is_none());
    assert!(value["accounts"][0].get("enabled").is_none());
    assert!(value["accounts"][0].get("allowedModels").is_none());
    assert_eq!(document.file_name, "zenith.json");
}

#[test]
fn access_token_only_accounts_export_in_every_supported_format() {
    let mut account = fixture();
    account.refresh_token = None;
    account.id_token = None;
    account.account_id = None;
    account.user_id = None;
    account.organization_id = None;
    for format in AccountExportFormat::all() {
        let document = build_account_export(
            format,
            std::slice::from_ref(&account),
            1_788_000_000_000,
            None,
        )
        .unwrap();
        document.validate().unwrap();
        assert!(document.content.contains(ACCESS), "{format:?}");
    }
}

#[test]
fn axon_hub_omits_a_missing_refresh_token() {
    let mut account = fixture();
    account.refresh_token = None;
    let document = build_account_export(
        AccountExportFormat::AxonHub,
        &[account],
        1_788_000_000_000,
        None,
    )
    .unwrap();
    let value: Value = serde_json::from_str(&document.content).unwrap();

    assert!(value["tokens"].get("refresh_token").is_none());
    assert!(!document.content.contains("__missing_refresh_token__"));
    assert!(value.get("axonhub_note").is_none());
}

#[test]
fn codex_export_preserves_the_required_null_api_key_field() {
    let document = build_account_export(
        AccountExportFormat::Codex,
        &[fixture()],
        1_788_000_000_000,
        None,
    )
    .unwrap();
    let value: Value = serde_json::from_str(&document.content).unwrap();
    assert!(value.get("OPENAI_API_KEY").is_some_and(Value::is_null));
}

#[test]
fn debug_and_validation_do_not_expose_exported_secrets() {
    let credential = fixture();
    let document = build_account_export(
        AccountExportFormat::Codex,
        std::slice::from_ref(&credential),
        1_788_000_000_000,
        None,
    )
    .unwrap();
    let debug = format!("{credential:?} {document:?}");
    for secret in [
        ACCESS,
        REFRESH,
        ID_TOKEN,
        "person@example.test",
        "account-secret-id",
    ] {
        assert!(!debug.contains(secret));
    }

    let invalid = AccountExportDocument {
        format: AccountExportFormat::Codex,
        account_count: 1,
        file_name: "../auth.json".into(),
        content: document.content,
    };
    assert!(invalid.validate().is_err());
}

#[test]
fn request_rejects_duplicates_and_unsafe_account_ids() {
    let valid = AccountExportRequest {
        account_ids: vec!["account_safe".into()],
        format: AccountExportFormat::Sub2api,
        description: None,
    };
    assert!(valid.validate().is_ok());
    assert!(AccountExportRequest {
        account_ids: vec!["account_safe".into()],
        format: AccountExportFormat::Zenith,
        description: Some("Seller description".into()),
    }
    .validate()
    .is_ok());
    assert!(AccountExportRequest {
        account_ids: vec!["account_safe".into()],
        format: AccountExportFormat::Sub2api,
        description: Some("Not supported here".into()),
    }
    .validate()
    .is_err());
    for account_ids in [
        Vec::new(),
        vec!["account_safe".into(), "account_safe".into()],
        vec!["../account".into()],
    ] {
        assert!(AccountExportRequest {
            account_ids,
            format: AccountExportFormat::Sub2api,
            description: None,
        }
        .validate()
        .is_err());
    }
}

fn fixture() -> AccountExportCredential {
    AccountExportCredential {
        oauth_client_kind: Default::default(),
        basis_points_headers: None,
        label: "Synthetic Plus".into(),
        email: Some("person@example.test".into()),
        phone: None,
        password: None,
        totp_secret: None,
        access_token: ACCESS.into(),
        refresh_token: Some(REFRESH.into()),
        id_token: Some(ID_TOKEN.into()),
        account_id: Some("account-secret-id".into()),
        user_id: Some("user-secret-id".into()),
        organization_id: Some("organization-secret-id".into()),
        plan_type: Some("plus".into()),
        expires_at_ms: Some(1_788_003_600_000),
        issued_at_ms: 1_788_000_000_000,
        subscription_active_until_ms: Some(1_900_000_000_000),
        created_at_ms: 1_787_000_000_000,
        priority: 10,
        enabled: true,
        tags: BTreeSet::from(["work".into(), "team".into()]),
    }
}

#[test]
fn export_carries_login_notes_only_when_present() {
    let mut account = fixture();
    account.phone = Some("950000000".into());
    account.password = Some("synthetic-password".into());
    account.totp_secret = Some("GEZDGNBVGY3TQOJQ".into());
    let document = build_account_export(
        AccountExportFormat::Cpa,
        std::slice::from_ref(&account),
        1_788_000_000_000,
        None,
    )
    .unwrap();
    assert!(document.content.contains("\"phone\": \"950000000\""));
    assert!(document
        .content
        .contains("\"password\": \"synthetic-password\""));
    assert!(document.content.contains("\"2fa\": \"GEZDGNBVGY3TQOJQ\""));
    let debug = format!("{account:?}");
    assert!(!debug.contains("synthetic-password"));
    assert!(!debug.contains("GEZDGNBVGY3TQOJQ"));

    let plain = build_account_export(
        AccountExportFormat::Cpa,
        &[fixture()],
        1_788_000_000_000,
        None,
    )
    .unwrap();
    assert!(!plain.content.contains("password"));
    assert!(!plain.content.contains("\"2fa\""));
    assert!(!plain.content.contains("phone"));
}

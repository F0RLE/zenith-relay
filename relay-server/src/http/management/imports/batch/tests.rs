use super::*;
use serde_json::json;
use zenith_relay_core::accounts::{ImportIssue, ImportWarningCode};

#[test]
fn shared_import_states_preserve_the_management_wire_contract() {
    let row = batch_preview_row(ImportPreviewRow {
        item_id: "item-synthetic".into(),
        source_file: None,
        label: "Synthetic account".into(),
        identity: "synthetic identity".into(),
        auth_mode: ImportAuthMode::OAuth,
        oauth_client_kind: Some(OAuthClientKind::Codex),
        source_name: "synthetic".into(),
        quota_status: ImportQuotaStatus::Failed,
        status: ImportPreviewStatus::QuotaFailed,
        plan: None,
        expires_at: None,
        subscription_expires_at: None,
        error: Some(ImportIssue {
            code: ImportIssueCode::QuotaProbeFailed,
            message: "Synthetic probe failure".into(),
        }),
        default_selected: false,
        selectable: true,
        existing: true,
        warnings: vec![
            ImportWarning {
                code: ImportWarningCode::AccessTokenOnly,
                count: None,
            },
            ImportWarning {
                code: ImportWarningCode::ProxiesIgnored,
                count: Some(2),
            },
        ],
    });
    let serialized = serde_json::to_value(row).unwrap();
    assert_eq!(serialized["authMode"], "oauth");
    assert_eq!(serialized["quotaStatus"], "failed");
    assert_eq!(serialized["status"], "quota_failed");
    assert_eq!(
        serialized["warnings"],
        json!([
            {"code": "access_token_only"}, {"code": "proxies_ignored", "count": 2}
        ])
    );
    assert_eq!(serialized["error"]["code"], "quota_probe_failed");
}

#[test]
fn invalid_preparation_preserves_the_unselectable_row_shape() {
    let row = invalid_shared_batch_row(
        "item-synthetic".into(),
        "Synthetic account".into(),
        "synthetic identity".into(),
        "synthetic".into(),
        "synthetic_error".into(),
        "Synthetic preparation failure".into(),
    );
    let serialized = serde_json::to_value(row).unwrap();
    assert_eq!(serialized["authMode"], "unknown");
    assert_eq!(serialized["quotaStatus"], "skipped");
    assert_eq!(serialized["status"], "invalid");
    assert_eq!(serialized["selectable"], false);
    assert_eq!(serialized["defaultSelected"], false);
    assert!(serialized.get("oauthClientKind").is_none());
    assert_eq!(serialized["error"]["code"], "synthetic_error");
}

#[test]
fn management_bundle_format_keeps_its_existing_name() {
    assert_eq!(
        import_format_name(ImportFormat::PortableAccountBundleV1),
        "portable_account_bundle"
    );
}

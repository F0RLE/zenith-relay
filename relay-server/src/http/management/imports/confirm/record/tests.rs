use super::*;
use crate::test_fixtures::synthetic_server_account;
use zenith_relay_core::providers::chatgpt::OAuthClientKind;

fn credential() -> AccountCredential {
    serde_json::from_value(serde_json::json!({
        "accessToken": "synthetic-access", "issuedAtMs": 1, "generation": 1,
        "chatgptAccountId": "synthetic-provider", "responsesUrl": "https://example.test/responses"
    }))
    .unwrap()
}

fn record_input<'a>(
    credential: &'a AccountCredential,
    existing: Option<&'a ServerAccountRecord>,
) -> ConfirmedAccountRecordInput<'a> {
    ConfirmedAccountRecordInput {
        preview: AccountImportPreview {
            session_id: "import_synthetic".into(),
            oauth_client_kind: credential.oauth_client_kind,
            account_id: "account-synthetic".into(),
            duplicate_account_id: None,
            label: "Preview label".into(),
            identity_hint: "Synthetic identity".into(),
            models: vec!["preview-model".into()],
            auth_state: AccountAuthState::Active,
            expires_at_ms: None,
            plan_type: None,
            subscription_active_until_ms: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            batch_session_id: None,
        },
        credential,
        existing,
        secret_ref: "account:synthetic-new".into(),
        created_at_ms: 10,
        observed_at_ms: 1_000,
        add_to_pool: false,
    }
}

#[test]
fn replacement_retains_operator_settings_and_does_not_opt_into_the_pool() {
    let credential = credential();
    let mut previous = synthetic_server_account("account-synthetic");
    previous.label = "Operator label".into();
    previous.enabled = false;
    previous.in_pool = false;
    previous.draining = true;
    previous.priority = 4;
    previous.weight = 3;
    previous.allowed_models = vec!["allowed-model".into()];
    previous.proxy_id = Some("proxy-synthetic".into());
    previous.purchase_cost_micro_usd = Some(25);
    let account = confirmed_account_record(record_input(&credential, Some(&previous))).unwrap();
    assert_eq!(account.label, previous.label);
    assert!(!account.enabled);
    assert!(!account.in_pool);
    assert!(account.draining);
    assert_eq!((account.priority, account.weight), (4, 3));
    assert_eq!(account.models, previous.models);
    assert_eq!(account.allowed_models, previous.allowed_models);
    assert_eq!(account.proxy_id, previous.proxy_id);
    assert_eq!(account.purchase_cost_micro_usd, Some(25));
    assert_eq!(account.created_at_ms, previous.created_at_ms);
    assert_eq!(account.secret_ref, "account:synthetic-new");
    assert_eq!(account.auth_state, AccountAuthState::DegradedAccessOnly);
}

#[test]
fn new_account_uses_preview_defaults_and_explicit_membership() {
    let mut credential = credential();
    credential.refresh_token = Some("synthetic-refresh".into());
    let mut input = record_input(&credential, None);
    input.add_to_pool = true;
    input.preview.plan_type = Some("plus".into());
    input.preview.subscription_active_until_ms = Some(10_000);
    let account = confirmed_account_record(input).unwrap();
    assert_eq!(account.label, "Preview label");
    assert!(account.enabled && account.in_pool);
    assert_eq!(account.models, ["preview-model"]);
    assert_eq!(account.created_at_ms, 10);
    assert_eq!(account.subscription.updated_at_ms, Some(1_000));
    assert_eq!(account.auth_state, AccountAuthState::Active);
}

#[test]
fn inherited_free_plan_is_still_rejected_for_bps_credentials() {
    let mut credential = credential();
    credential.oauth_client_kind = OAuthClientKind::ExcelBps;
    let mut previous = synthetic_server_account("account-synthetic");
    previous.subscription.plan_type = Some("free".into());
    let result = confirmed_account_record(record_input(&credential, Some(&previous)));
    assert!(matches!(result, Err(error)
        if error.code == error_codes::BASIS_POINTS_FREE_ACCOUNT_UNSUPPORTED));
}

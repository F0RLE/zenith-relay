use super::build::{object, optional_timestamp, strip_nulls, timestamp};
use super::{AccountExportCredential, AccountExportFormat};
use crate::error_codes;
use crate::Result;
use serde_json::{json, Map, Value};
struct AccountExportValues<'a> {
    account: &'a AccountExportCredential,
    exported_at: &'a str,
    expires_at: Option<String>,
    subscription_expires_at: Option<String>,
    created_at: String,
    issued_at: String,
    expires_in: Option<u64>,
}

impl<'a> AccountExportValues<'a> {
    fn new(
        account: &'a AccountExportCredential,
        exported_at_ms: u64,
        exported_at: &'a str,
    ) -> Result<Self> {
        Ok(Self {
            account,
            exported_at,
            expires_at: optional_timestamp(account.expires_at_ms)?,
            subscription_expires_at: optional_timestamp(account.subscription_active_until_ms)?,
            created_at: timestamp(if account.created_at_ms == 0 {
                account.issued_at_ms
            } else {
                account.created_at_ms
            })?,
            issued_at: timestamp(account.issued_at_ms)?,
            expires_in: account
                .expires_at_ms
                .and_then(|expires_at| expires_at.checked_sub(exported_at_ms))
                .map(|milliseconds| milliseconds / 1_000),
        })
    }
}

pub(super) fn account_value(
    format: AccountExportFormat,
    account: &AccountExportCredential,
    exported_at_ms: u64,
    exported_at: &str,
) -> Result<Value> {
    let values = AccountExportValues::new(account, exported_at_ms, exported_at)?;
    let mut value = match format {
        AccountExportFormat::Zenith => zenith_account_value(&values),
        AccountExportFormat::Cpa => cpa_account_value(&values),
        AccountExportFormat::Sub2api => sub2api_account_value(&values),
        AccountExportFormat::NineRouter => nine_router_account_value(&values),
        AccountExportFormat::Codex => codex_account_value(&values),
        AccountExportFormat::AxonHub => axon_hub_account_value(&values),
        AccountExportFormat::CodexManager => codex_manager_account_value(&values),
    };
    attach_login_notes(&mut value, account);
    Ok(if format == AccountExportFormat::Codex {
        value
    } else {
        strip_nulls(value)
    })
}

fn attach_login_notes(value: &mut Value, account: &AccountExportCredential) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    insert_login_note(object, "phone", account.phone.as_deref());
    insert_login_note(object, "password", account.password.as_deref());
    insert_login_note(object, "2fa", account.totp_secret.as_deref());
}

fn insert_login_note(object: &mut Map<String, Value>, key: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        object.insert(key.to_string(), Value::String(value.to_string()));
    }
}

fn zenith_account_value(values: &AccountExportValues<'_>) -> Value {
    let account = values.account;
    json!({
        "name": account.label,
        "provider": "openai",
        "auth": {
            "type": "oauth",
            "accessToken": account.access_token,
            "refreshToken": account.refresh_token,
            "idToken": account.id_token,
            "issuedAt": values.issued_at,
            "expiresAt": values.expires_at,
        },
        "identity": {
            "email": account.email,
            "accountId": account.account_id,
            "userId": account.user_id,
            "organizationId": account.organization_id,
        },
        "subscription": {
            "plan": account.plan_type,
            "expiresAt": values.subscription_expires_at,
        },
    })
}

fn cpa_account_value(values: &AccountExportValues<'_>) -> Value {
    let account = values.account;
    json!({
        "type": "codex",
        "account_id": account.account_id,
        "chatgpt_account_id": account.account_id,
        "email": account.email,
        "name": account.label,
        "plan_type": account.plan_type,
        "chatgpt_plan_type": account.plan_type,
        "id_token": account.id_token,
        "access_token": account.access_token,
        "refresh_token": account.refresh_token.as_deref().unwrap_or(""),
        "last_refresh": values.exported_at,
        error_codes::EXPIRED: values.expires_at,
        "disabled": (!account.enabled).then_some(true),
    })
}

fn sub2api_account_value(values: &AccountExportValues<'_>) -> Value {
    let account = values.account;
    json!({
        "name": account.label,
        "platform": "openai",
        "type": "oauth",
        "credentials": {
            "access_token": account.access_token,
            "expires_at": values.expires_at,
            "refresh_token": account.refresh_token,
            "id_token": account.id_token,
            "email": account.email,
            "chatgpt_account_id": account.account_id,
            "chatgpt_user_id": account.user_id,
            "organization_id": account.organization_id,
            "plan_type": account.plan_type,
            "subscription_expires_at": values.subscription_expires_at,
        },
        "concurrency": 0,
        "priority": account.priority,
    })
}

fn nine_router_account_value(values: &AccountExportValues<'_>) -> Value {
    let account = values.account;
    json!({
        "accessToken": account.access_token,
        "refreshToken": account.refresh_token,
        "expiresAt": values.expires_at,
        "testStatus": "active",
        "expiresIn": values.expires_in,
        "providerSpecificData": {
            "chatgptAccountId": account.account_id,
            "chatgptUserId": account.user_id,
            "chatgptPlanType": account.plan_type,
        },
        "id": account.account_id,
        "provider": "codex",
        "authType": "oauth",
        "name": account.label,
        "email": account.email,
        "priority": account.priority,
        "isActive": account.enabled,
        "createdAt": values.created_at,
        "updatedAt": values.exported_at,
    })
}

fn codex_account_value(values: &AccountExportValues<'_>) -> Value {
    let account = values.account;
    let mut root = object(json!({
        "auth_mode": "chatgpt",
        "tokens": {
            "id_token": account.id_token.as_deref().unwrap_or(""),
            "access_token": account.access_token,
            "refresh_token": account.refresh_token.as_deref().unwrap_or(""),
            "account_id": account.account_id.as_deref().unwrap_or(""),
        },
        "last_refresh": values.exported_at,
    }));
    root.insert("OPENAI_API_KEY".to_string(), Value::Null);
    Value::Object(root)
}

fn axon_hub_account_value(values: &AccountExportValues<'_>) -> Value {
    let account = values.account;
    json!({
        "auth_mode": "chatgpt",
        "last_refresh": values.issued_at,
        "tokens": {
            "access_token": account.access_token,
            "refresh_token": account.refresh_token,
            "id_token": account.id_token.as_deref().unwrap_or(""),
        },
    })
}

fn codex_manager_account_value(values: &AccountExportValues<'_>) -> Value {
    let account = values.account;
    json!({
        "tokens": {
            "access_token": account.access_token,
            "refresh_token": account.refresh_token.as_deref().unwrap_or(""),
            "id_token": account.id_token.as_deref().unwrap_or(""),
            "account_id": account.account_id,
            "chatgpt_account_id": account.account_id,
        },
        "meta": {
            "label": account.label,
            "chatgpt_account_id": account.account_id,
        },
    })
}

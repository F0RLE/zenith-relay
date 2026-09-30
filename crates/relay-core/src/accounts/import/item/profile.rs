use super::super::sanitization::*;
use super::super::*;
use super::jwt::ImportedJwtMetadata;
use super::vocabulary::*;
use crate::accounts::{
    normalize_login_password, normalize_login_phone, normalize_login_totp_secret,
};

pub(super) struct ImportFieldSources<'a> {
    pub(super) object: &'a Map<String, Value>,
    pub(super) credentials: &'a Map<String, Value>,
    pub(super) agent_identity: Option<&'a Map<String, Value>>,
    pub(super) account: Option<&'a Map<String, Value>>,
    pub(super) provider: Option<&'a Map<String, Value>>,
    pub(super) identity: Option<&'a Map<String, Value>>,
    pub(super) meta: Option<&'a Map<String, Value>>,
    pub(super) subscription: Option<&'a Map<String, Value>>,
    pub(super) user: Option<&'a Map<String, Value>>,
    pub(super) session_profile: Option<&'a Map<String, Value>>,
    pub(super) header_account_id: Option<&'a str>,
}

pub(super) struct ImportProfile {
    pub(super) email: Option<String>,
    pub(super) phone: Option<String>,
    pub(super) password: Option<String>,
    pub(super) totp_secret: Option<String>,
    pub(super) account_id: Option<String>,
    pub(super) chatgpt_user_id: Option<String>,
    pub(super) organization_id: Option<String>,
    pub(super) plan: Option<String>,
    pub(super) expires_at: Option<String>,
    pub(super) subscription_expires_at: Option<String>,
    pub(super) base_url: Option<String>,
    pub(super) base_url_supplied: bool,
    pub(super) protocol: Option<String>,
    pub(super) protocol_supplied: bool,
    pub(super) priority: Option<i32>,
    pub(super) account_is_fedramp: bool,
    pub(super) metadata_rejected: bool,
}

pub(super) fn read_import_profile(
    sources: ImportFieldSources<'_>,
    jwt: ImportedJwtMetadata,
) -> ImportProfile {
    let ImportFieldSources {
        object,
        credentials,
        agent_identity: agent_identity_data,
        account,
        provider: provider_data,
        identity,
        meta,
        subscription,
        user,
        session_profile,
        header_account_id,
    } = sources;
    let email = credential_string(object, credentials, None, EMAIL_FIELDS)
        .or_else(|| {
            first_string_field(&[
                (agent_identity_data, EMAIL_FIELDS),
                (account, EMAIL_FIELDS),
                (user, EMAIL_FIELDS),
                (session_profile, EMAIL_FIELDS),
                (provider_data, EMAIL_FIELDS),
                (identity, EMAIL_FIELDS),
            ])
            .map(str::to_string)
        })
        .or(jwt.email);
    let note_sources = [
        agent_identity_data,
        account,
        user,
        session_profile,
        provider_data,
        identity,
        meta,
    ];
    let phone = login_note(
        object,
        credentials,
        &note_sources,
        PHONE_FIELDS,
        normalize_login_phone,
    );
    let password = login_note(
        object,
        credentials,
        &note_sources,
        PASSWORD_FIELDS,
        normalize_login_password,
    );
    let totp_secret = login_note(
        object,
        credentials,
        &note_sources,
        TOTP_SECRET_FIELDS,
        normalize_login_totp_secret,
    );
    let account_id_value =
        credential_str(object, credentials, None, ACCOUNT_ID_FIELDS).or_else(|| {
            first_string_field(&[
                (agent_identity_data, ACCOUNT_ID_FIELDS),
                (account, &["id"]),
                (account, ACCOUNT_ID_FIELDS),
                (provider_data, ACCOUNT_ID_FIELDS),
                (meta, ACCOUNT_ID_FIELDS),
                (identity, ACCOUNT_ID_FIELDS),
            ])
        });
    let account_id = safe_identifier(account_id_value)
        .or_else(|| safe_identifier(header_account_id))
        .or(jwt.account_id);
    let profile_sources = [
        (agent_identity_data, USER_ID_FIELDS),
        (account, USER_ID_FIELDS),
        (provider_data, USER_ID_FIELDS),
        (meta, USER_ID_FIELDS),
        (identity, USER_ID_FIELDS),
    ];
    let chatgpt_user_id_value = credential_str(object, credentials, None, USER_ID_FIELDS)
        .or_else(|| first_string_field(&profile_sources));
    let chatgpt_user_id = safe_identifier(chatgpt_user_id_value)
        .or_else(|| safe_identifier(user.and_then(|user| string_field(user, &["id"]))))
        .or(jwt.user_id);
    let organization_sources = [
        (agent_identity_data, ORGANIZATION_ID_FIELDS),
        (account, ORGANIZATION_ID_FIELDS),
        (provider_data, ORGANIZATION_ID_FIELDS),
        (meta, ORGANIZATION_ID_FIELDS),
        (identity, ORGANIZATION_ID_FIELDS),
    ];
    let organization_id_value = credential_str(object, credentials, None, ORGANIZATION_ID_FIELDS)
        .or_else(|| first_string_field(&organization_sources));
    let organization_id = safe_identifier(organization_id_value);
    let plan_value = credential_value(object, credentials, None, PLAN_FIELDS).or_else(|| {
        first_value_field(&[
            (agent_identity_data, PLAN_FIELDS),
            (account, PLAN_FIELDS),
            (provider_data, PLAN_FIELDS),
            (meta, PLAN_FIELDS),
            (subscription, PLAN_FIELDS),
        ])
    });
    let plan = safe_metadata(plan_value.and_then(Value::as_str)).or(jwt.plan_type);
    let expires_at_value = credential_value(object, credentials, None, EXPIRES_AT_FIELDS)
        .or_else(|| provider_data.and_then(|data| value_field(data, EXPIRES_AT_FIELDS)));
    let expires_at = safe_expiry(expires_at_value).or(jwt.expires_at);
    let subscription_expires_at_value =
        credential_value(object, credentials, None, SUBSCRIPTION_EXPIRES_AT_FIELDS).or_else(|| {
            first_value_field(&[
                (account, SUBSCRIPTION_EXPIRES_AT_FIELDS),
                (provider_data, SUBSCRIPTION_EXPIRES_AT_FIELDS),
                (subscription, &["expiresAt", "expires_at"]),
            ])
        });
    let subscription_expires_at =
        safe_expiry(subscription_expires_at_value).or(jwt.subscription_expires_at);
    let base_url_value = credential_value(
        object,
        credentials,
        None,
        &[
            "base_url",
            "baseUrl",
            "api_base",
            "apiBase",
            "api_base_url",
            "apiBaseUrl",
        ],
    );
    let base_url_supplied = base_url_value.is_some();
    let base_url = safe_base_url(base_url_value.and_then(Value::as_str));
    let protocol_value = credential_value(
        object,
        credentials,
        None,
        &[
            "protocol",
            "wire_api",
            "wireApi",
            "api_wire_api",
            "apiWireApi",
        ],
    );
    let protocol_supplied = protocol_value.is_some();
    let protocol = safe_protocol(protocol_value.and_then(Value::as_str));
    let priority = credential_value(object, credentials, None, &["priority"])
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok());
    let account_is_fedramp = credential_bool(
        object,
        credentials,
        &["chatgpt_account_is_fedramp", "chatgptAccountIsFedramp"],
    )
    .or_else(|| {
        agent_identity_data.and_then(|data| {
            value_field(
                data,
                &["chatgpt_account_is_fedramp", "chatgptAccountIsFedramp"],
            )
            .and_then(Value::as_bool)
        })
    })
    .unwrap_or(false);
    let metadata_rejected = metadata_was_rejected(
        base_url_value,
        base_url.as_deref(),
        protocol_value,
        protocol.as_deref(),
        plan_value,
        plan.as_deref(),
    );
    ImportProfile {
        email,
        phone,
        password,
        totp_secret,
        account_id,
        chatgpt_user_id,
        organization_id,
        plan,
        expires_at,
        subscription_expires_at,
        base_url,
        base_url_supplied,
        protocol,
        protocol_supplied,
        priority,
        account_is_fedramp,
        metadata_rejected,
    }
}

fn login_note(
    object: &Map<String, Value>,
    credentials: &Map<String, Value>,
    extras: &[Option<&Map<String, Value>>],
    fields: &[&str],
    normalize: fn(&str) -> Option<String>,
) -> Option<String> {
    credential_string(object, credentials, None, fields)
        .or_else(|| {
            extras.iter().find_map(|source| {
                source
                    .and_then(|data| string_field(data, fields))
                    .map(str::to_string)
            })
        })
        .and_then(|value| normalize(&value))
}

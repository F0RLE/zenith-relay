use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{TimeZone, Utc};
use serde::Deserialize;

const MAX_JWT_BYTES: usize = 64 * 1024;
const MAX_JWT_PAYLOAD_BYTES: usize = 16 * 1024;

#[derive(Default, Deserialize)]
pub(super) struct ImportedJwtClaims {
    #[serde(default)]
    pub(super) email: Option<String>,
    #[serde(default)]
    pub(super) exp: Option<u64>,
    #[serde(rename = "https://api.openai.com/profile", default)]
    pub(super) profile: Option<ImportedProfileClaims>,
    #[serde(rename = "https://api.openai.com/auth", default)]
    pub(super) auth: Option<ImportedAuthClaims>,
}

#[derive(Default, Deserialize)]
pub(super) struct ImportedProfileClaims {
    #[serde(default)]
    pub(super) email: Option<String>,
}

#[derive(Default, Deserialize)]
pub(super) struct ImportedAuthClaims {
    #[serde(default)]
    pub(super) chatgpt_plan_type: Option<String>,
    #[serde(default)]
    pub(super) chatgpt_subscription_active_until: Option<serde_json::Value>,
    #[serde(default)]
    pub(super) chatgpt_user_id: Option<String>,
    #[serde(default)]
    pub(super) user_id: Option<String>,
    #[serde(default)]
    pub(super) chatgpt_account_id: Option<String>,
    #[serde(default)]
    pub(super) account_id: Option<String>,
    #[serde(default)]
    pub(super) chatgpt_account_is_fedramp: bool,
}

#[derive(Default)]
pub(in crate::local_pool::accounts) struct ImportedIdentity {
    pub(in crate::local_pool::accounts) email: Option<String>,
    pub(in crate::local_pool::accounts) plan_type: Option<String>,
    pub(in crate::local_pool::accounts) subscription_active_until_ms: Option<u64>,
    pub(in crate::local_pool::accounts) provider_user_id: Option<String>,
    pub(in crate::local_pool::accounts) provider_account_id: Option<String>,
    pub(in crate::local_pool::accounts) account_is_fedramp: bool,
    pub(in crate::local_pool::accounts) access_expires_at_ms: Option<u64>,
}

pub(in crate::local_pool::accounts) fn imported_identity(
    id_token: Option<&str>,
    access_token: Option<&str>,
) -> ImportedIdentity {
    let id_claims = id_token.and_then(decode_imported_jwt);
    let access_claims = access_token.and_then(decode_imported_jwt);
    let id_auth = id_claims.as_ref().and_then(|claims| claims.auth.as_ref());
    let access_auth = access_claims
        .as_ref()
        .and_then(|claims| claims.auth.as_ref());
    ImportedIdentity {
        email: claim_email(id_claims.as_ref()).or_else(|| claim_email(access_claims.as_ref())),
        plan_type: auth_string(id_auth, |auth| &auth.chatgpt_plan_type)
            .or_else(|| auth_string(access_auth, |auth| &auth.chatgpt_plan_type)),
        subscription_active_until_ms: id_auth
            .and_then(|auth| auth.chatgpt_subscription_active_until.as_ref())
            .and_then(parse_subscription_timestamp_value_ms)
            .or_else(|| {
                access_auth
                    .and_then(|auth| auth.chatgpt_subscription_active_until.as_ref())
                    .and_then(parse_subscription_timestamp_value_ms)
            }),
        provider_user_id: auth_string(id_auth, |auth| &auth.chatgpt_user_id)
            .or_else(|| auth_string(id_auth, |auth| &auth.user_id))
            .or_else(|| auth_string(access_auth, |auth| &auth.chatgpt_user_id))
            .or_else(|| auth_string(access_auth, |auth| &auth.user_id)),
        provider_account_id: auth_string(access_auth, |auth| &auth.chatgpt_account_id)
            .or_else(|| auth_string(access_auth, |auth| &auth.account_id))
            .or_else(|| auth_string(id_auth, |auth| &auth.chatgpt_account_id))
            .or_else(|| auth_string(id_auth, |auth| &auth.account_id)),
        account_is_fedramp: id_auth
            .or(access_auth)
            .is_some_and(|auth| auth.chatgpt_account_is_fedramp),
        access_expires_at_ms: access_claims
            .and_then(|claims| claims.exp)
            .map(|seconds| seconds.saturating_mul(1_000)),
    }
}

pub(super) fn parse_subscription_timestamp_value_ms(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(value) => value.as_u64().and_then(normalize_epoch_timestamp_ms),
        serde_json::Value::String(value) => parse_subscription_timestamp_ms(value),
        _ => None,
    }
}

pub(in crate::local_pool::accounts) fn parse_subscription_timestamp_ms(value: &str) -> Option<u64> {
    let value = value.trim();
    if value.is_empty() || value.len() > 64 {
        return None;
    }
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value
            .parse::<u64>()
            .ok()
            .and_then(normalize_epoch_timestamp_ms);
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .and_then(|value| u64::try_from(value.timestamp_millis()).ok())
}

pub(super) fn normalize_epoch_timestamp_ms(value: u64) -> Option<u64> {
    let value = if value < 100_000_000_000 {
        value.checked_mul(1_000)?
    } else {
        value
    };
    i64::try_from(value)
        .ok()
        .and_then(|value| Utc.timestamp_millis_opt(value).single())
        .map(|_| value)
}

pub(super) fn decode_imported_jwt(token: &str) -> Option<ImportedJwtClaims> {
    if token.is_empty() || token.len() > MAX_JWT_BYTES {
        return None;
    }
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if header.is_empty() || payload.is_empty() || signature.is_empty() {
        return None;
    }
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    if decoded.len() > MAX_JWT_PAYLOAD_BYTES {
        return None;
    }
    serde_json::from_slice(&decoded).ok()
}

pub(super) fn claim_email(claims: Option<&ImportedJwtClaims>) -> Option<String> {
    claims.and_then(|claims| {
        nonempty(claims.email.clone()).or_else(|| {
            claims
                .profile
                .as_ref()
                .and_then(|profile| nonempty(profile.email.clone()))
        })
    })
}

pub(super) fn auth_string(
    auth: Option<&ImportedAuthClaims>,
    select: impl for<'a> Fn(&'a ImportedAuthClaims) -> &'a Option<String>,
) -> Option<String> {
    auth.and_then(|auth| nonempty(select(auth).clone()))
}

pub(super) fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

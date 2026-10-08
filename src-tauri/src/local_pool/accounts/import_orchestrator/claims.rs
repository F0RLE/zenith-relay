use serde::Deserialize;
use zenith_relay_core::accounts::decode_unverified_jwt_payload;
use zenith_relay_core::omit_blank;

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
    /// Account ids found in the import document's JWTs. JWTs are unsigned
    /// hints; the importer must reconcile every hint with an authenticated
    /// account-check response before persisting credentials.
    pub(in crate::local_pool::accounts) account_id_hints: Vec<String>,
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
    let mut account_id_hints = Vec::new();
    for auth in [access_auth, id_auth].into_iter().flatten() {
        for account_id_hint in [&auth.chatgpt_account_id, &auth.account_id]
            .into_iter()
            .filter_map(|account_id_hint| omit_blank(account_id_hint.clone()))
        {
            if !account_id_hints
                .iter()
                .any(|existing: &String| existing.eq_ignore_ascii_case(&account_id_hint))
            {
                account_id_hints.push(account_id_hint);
            }
        }
    }
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
        provider_account_id: account_id_hints.first().cloned(),
        account_id_hints,
        account_is_fedramp: id_auth
            .or(access_auth)
            .is_some_and(|auth| auth.chatgpt_account_is_fedramp),
        access_expires_at_ms: access_claims
            .and_then(|claims| claims.exp)
            .map(|seconds| seconds.saturating_mul(1_000)),
    }
}

pub(super) fn parse_subscription_timestamp_value_ms(
    timestamp_value: &serde_json::Value,
) -> Option<u64> {
    zenith_relay_core::providers::chatgpt::parse_subscription_timestamp_ms(timestamp_value)
}

pub(in crate::local_pool::accounts) fn parse_subscription_timestamp_ms(
    timestamp_text: &str,
) -> Option<u64> {
    zenith_relay_core::providers::chatgpt::parse_subscription_timestamp_text(timestamp_text)
}

pub(super) fn decode_imported_jwt(token: &str) -> Option<ImportedJwtClaims> {
    decode_unverified_jwt_payload(token)
}

pub(super) fn claim_email(claims: Option<&ImportedJwtClaims>) -> Option<String> {
    claims.and_then(|claims| {
        omit_blank(claims.email.clone()).or_else(|| {
            claims
                .profile
                .as_ref()
                .and_then(|profile| omit_blank(profile.email.clone()))
        })
    })
}

pub(super) fn auth_string(
    auth: Option<&ImportedAuthClaims>,
    select: impl for<'a> Fn(&'a ImportedAuthClaims) -> &'a Option<String>,
) -> Option<String> {
    auth.and_then(|auth| omit_blank(select(auth).clone()))
}

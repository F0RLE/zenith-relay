use super::super::sanitization::{safe_expiry, safe_identifier, safe_metadata, string_field};
use crate::accounts::normalize_login_email;
use serde_json::Value;

/// Import identity key for one ChatGPT account.
/// The seed prefers account plus email, then account plus user, then the
/// account alone. Email or user alone is used only when the account id is
/// absent. The hashed value is what parsed import rows store as `identity_key`.
pub fn chatgpt_token_identity_key(
    account_id: Option<&str>,
    user_id: Option<&str>,
    email: Option<&str>,
) -> Option<String> {
    token_identity_seed(account_id, user_id, email)
        .map(|seed| super::super::sanitization::sha256_hex(&format!("account:{seed}"), None, None))
}

pub(in crate::accounts::import::item) fn token_identity_seed(
    account_id: Option<&str>,
    user_id: Option<&str>,
    email: Option<&str>,
) -> Option<String> {
    let account = account_id.map(|value| value.trim().to_ascii_lowercase());
    let user = user_id.map(|value| value.trim().to_ascii_lowercase());
    let email = email.map(|value| value.trim().to_ascii_lowercase());
    match (account, email, user) {
        (Some(account), Some(email), _) => Some(format!("account:{account}:email:{email}")),
        (Some(account), None, Some(user)) => Some(format!("account:{account}:user:{user}")),
        (Some(account), None, None) => Some(format!("account:{account}")),
        (None, Some(email), _) => Some(format!("email:{email}")),
        (None, None, Some(user)) => Some(format!("user:{user}")),
        (None, None, None) => None,
    }
}

#[derive(Default)]
pub(in crate::accounts::import::item) struct ImportedJwtMetadata {
    pub(in crate::accounts::import::item) email: Option<String>,
    pub(in crate::accounts::import::item) account_id: Option<String>,
    pub(in crate::accounts::import::item) user_id: Option<String>,
    pub(in crate::accounts::import::item) plan_type: Option<String>,
    pub(in crate::accounts::import::item) expires_at: Option<String>,
    pub(in crate::accounts::import::item) subscription_expires_at: Option<String>,
}

pub(in crate::accounts::import::item) fn imported_jwt_metadata(
    id_token: Option<&str>,
    access_token: Option<&str>,
) -> ImportedJwtMetadata {
    let mut metadata = ImportedJwtMetadata::default();
    for token in [id_token, access_token].into_iter().flatten() {
        let Some(claims) =
            crate::accounts::decode_unverified_jwt_payload::<Value>(token).filter(Value::is_object)
        else {
            continue;
        };
        let auth = claims
            .get("https://api.openai.com/auth")
            .and_then(Value::as_object);
        let profile = claims
            .get("https://api.openai.com/profile")
            .and_then(Value::as_object);
        metadata.email = metadata
            .email
            .or_else(|| jwt_email(claims.get("email")))
            .or_else(|| profile.and_then(|profile| jwt_email(profile.get("email"))));
        metadata.account_id = metadata.account_id.or_else(|| {
            auth.and_then(|auth| {
                safe_identifier(string_field(auth, &["chatgpt_account_id", "account_id"]))
            })
        });
        metadata.user_id = metadata.user_id.or_else(|| {
            auth.and_then(|auth| {
                safe_identifier(string_field(auth, &["chatgpt_user_id", "user_id"]))
            })
        });
        metadata.plan_type = metadata.plan_type.or_else(|| {
            auth.and_then(|auth| {
                safe_metadata(auth.get("chatgpt_plan_type").and_then(Value::as_str))
            })
        });
        metadata.subscription_expires_at = metadata.subscription_expires_at.or_else(|| {
            auth.and_then(|auth| safe_expiry(auth.get("chatgpt_subscription_active_until")))
        });
        metadata.expires_at = metadata
            .expires_at
            .or_else(|| safe_expiry(claims.get("exp")));
    }
    metadata
}

fn jwt_email(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .and_then(normalize_login_email)
}

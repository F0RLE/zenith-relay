use chrono::{TimeZone, Utc};
use sha2::{Digest, Sha256};

pub(in crate::local_pool::accounts) fn masked_account_identity(value: &str) -> String {
    let suffix = value
        .chars()
        .rev()
        .filter(|character| character.is_ascii_alphanumeric())
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    if suffix.is_empty() {
        "Account [redacted]".into()
    } else {
        format!("Account ****{suffix}")
    }
}

pub(in crate::local_pool::accounts) fn timestamp_from_ms(value: u64) -> Option<String> {
    let value = i64::try_from(value).ok()?;
    Utc.timestamp_millis_opt(value)
        .single()
        .map(|value| value.to_rfc3339())
}

#[cfg(test)]
pub(in crate::local_pool::accounts) fn account_id_from_check_response(
    payload: &serde_json::Value,
) -> Option<String> {
    zenith_relay_core::providers::chatgpt::account_ids_from_check_response(payload)
        .into_iter()
        .next()
}

#[cfg(test)]
pub(in crate::local_pool::accounts) fn normalized_profile_account_id(
    value: &str,
) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control))
        .then(|| value.to_string())
}

pub(in crate::local_pool::accounts) fn provider_identity_key(
    provider_account_id: &str,
    provider_user_id: Option<&str>,
    email: Option<&str>,
) -> String {
    let account = provider_account_id.trim().to_ascii_lowercase();
    let user = provider_user_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let email = email
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase);
    let identity = match (email, user) {
        (Some(email), _) => format!("account:{account}:email:{email}"),
        (None, Some(user)) => format!("account:{account}:user:{user}"),
        (None, None) => format!("account:{account}"),
    };
    hex::encode(Sha256::digest(format!("account:{identity}").as_bytes()))
}

use chrono::{TimeZone, Utc};

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
    zenith_relay_core::accounts::chatgpt_token_identity_key(
        Some(provider_account_id),
        nonempty(provider_user_id),
        nonempty(email),
    )
    .expect("provider account id produces an import identity")
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    match value.map(str::trim) {
        Some(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

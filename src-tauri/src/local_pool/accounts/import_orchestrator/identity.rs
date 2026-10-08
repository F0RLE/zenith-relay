use chrono::{TimeZone, Utc};

pub(in crate::local_pool::accounts) fn masked_account_identity(account_identifier: &str) -> String {
    let suffix = account_identifier
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

pub(in crate::local_pool::accounts) fn timestamp_from_ms(timestamp_ms: u64) -> Option<String> {
    let timestamp_ms = i64::try_from(timestamp_ms).ok()?;
    Utc.timestamp_millis_opt(timestamp_ms)
        .single()
        .map(|timestamp| timestamp.to_rfc3339())
}

#[cfg(test)]
pub(in crate::local_pool::accounts) fn account_id_from_check_response(
    check_response: &serde_json::Value,
) -> Option<String> {
    zenith_relay_core::providers::chatgpt::account_ids_from_check_response(check_response)
        .into_iter()
        .next()
}

#[cfg(test)]
pub(in crate::local_pool::accounts) fn normalized_profile_account_id(
    account_id: &str,
) -> Option<String> {
    let account_id = account_id.trim();
    (!account_id.is_empty() && account_id.len() <= 512 && !account_id.chars().any(char::is_control))
        .then(|| account_id.to_string())
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

fn nonempty(optional_text: Option<&str>) -> Option<&str> {
    match optional_text.map(str::trim) {
        Some(text) if !text.is_empty() => Some(text),
        _ => None,
    }
}

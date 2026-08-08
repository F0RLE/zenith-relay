use super::*;
use sha2::{Digest, Sha256};
use url::Url;

pub(super) fn credential_string(
    object: &Map<String, Value>,
    credentials: &Map<String, Value>,
    tokens: Option<&Map<String, Value>>,
    fields: &[&str],
) -> Option<String> {
    credential_str(object, credentials, tokens, fields)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(super) fn credential_str<'a>(
    object: &'a Map<String, Value>,
    credentials: &'a Map<String, Value>,
    tokens: Option<&'a Map<String, Value>>,
    fields: &[&str],
) -> Option<&'a str> {
    tokens
        .and_then(|tokens| string_field(tokens, fields))
        .or_else(|| string_field(credentials, fields))
        .or_else(|| string_field(object, fields))
}

pub(super) fn credential_value<'a>(
    object: &'a Map<String, Value>,
    credentials: &'a Map<String, Value>,
    tokens: Option<&'a Map<String, Value>>,
    fields: &[&str],
) -> Option<&'a Value> {
    tokens
        .and_then(|tokens| value_field(tokens, fields))
        .or_else(|| value_field(credentials, fields))
        .or_else(|| value_field(object, fields))
}

pub(super) fn string_field<'a>(object: &'a Map<String, Value>, fields: &[&str]) -> Option<&'a str> {
    value_field(object, fields)?.as_str()
}

pub(super) fn value_field<'a>(
    object: &'a Map<String, Value>,
    fields: &[&str],
) -> Option<&'a Value> {
    fields.iter().find_map(|field| object.get(*field))
}

pub(super) fn safe_identifier(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty()
        || value.len() > 256
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
    {
        None
    } else {
        Some(value.to_string())
    }
}

pub(super) fn safe_metadata(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b' '))
    {
        None
    } else {
        Some(value.to_string())
    }
}

pub(super) fn safe_expiry(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::Number(value) => Some(value.to_string()),
        Value::String(value)
            if !value.is_empty()
                && value.len() <= 64
                && value.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b':' | b'+' | b'.' | b' ')
                }) =>
        {
            Some(value.to_string())
        }
        _ => None,
    }
}

pub(super) fn safe_base_url(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || value.len() > 2048 {
        return None;
    }
    let mut parsed = Url::parse(value).ok()?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return None;
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    Some(parsed.to_string().trim_end_matches('/').to_string())
}

pub(super) fn safe_protocol(value: Option<&str>) -> Option<String> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "responses" => Some("responses".to_string()),
        "chat_completions" | "chat-completions" | "chat" => Some("chat_completions".to_string()),
        _ => None,
    }
}

pub(super) fn metadata_was_rejected(
    object: &Map<String, Value>,
    base_url: Option<&str>,
    protocol: Option<&str>,
    plan_value: Option<&Value>,
    plan: Option<&str>,
) -> bool {
    (value_field(object, &["base_url", "baseUrl", "api_base", "apiBase"]).is_some()
        && base_url.is_none())
        || (value_field(object, &["protocol", "wire_api", "wireApi"]).is_some()
            && protocol.is_none())
        || (plan_value.is_some() && plan.is_none())
}

pub(super) fn safe_label(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() || value.chars().count() > 80 || value.chars().any(char::is_control) {
        return None;
    }
    Some(if value.contains('@') {
        mask_email(value)
    } else {
        value.to_string()
    })
}

pub(super) fn redact_label_secrets<'a>(
    label: String,
    secrets: impl IntoIterator<Item = Option<&'a str>>,
    masked_identity: &str,
) -> String {
    if secrets
        .into_iter()
        .flatten()
        .filter(|secret| secret.len() >= 4)
        .any(|secret| label.contains(secret))
    {
        masked_identity.to_string()
    } else {
        label
    }
}

pub(super) fn redact_optional_metadata(
    value: &mut Option<String>,
    sensitive_values: &[Option<&str>],
) {
    if value.as_ref().is_some_and(|value| {
        sensitive_values
            .iter()
            .flatten()
            .filter(|sensitive| sensitive.len() >= 4)
            .any(|sensitive| value.contains(sensitive))
    }) {
        *value = None;
    }
}

pub(super) fn redact_file_name(value: &str) -> String {
    let (stem, extension) = value
        .rsplit_once('.')
        .filter(|(_, extension)| {
            !extension.is_empty()
                && extension.len() <= 8
                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .map_or((value, None), |(stem, extension)| (stem, Some(extension)));
    let lower = stem.to_ascii_lowercase();
    let sensitive = stem.contains('@')
        || lower.starts_with("sk-")
        || lower.starts_with("eyj")
        || lower.starts_with("access-")
        || lower.starts_with("refresh-")
        || lower.starts_with("token-")
        || lower.contains("access_token")
        || lower.contains("refresh_token")
        || lower.contains("api_key")
        || stem.len() > 64;
    let stem = if stem.contains('@') {
        mask_email(stem)
    } else if sensitive {
        "import".to_string()
    } else {
        stem.to_string()
    };
    extension.map_or(stem.clone(), |extension| format!("{stem}.{extension}"))
}

pub(super) fn redact_file_name_with(value: &str, sensitive_values: &[Option<&str>]) -> String {
    if sensitive_values
        .iter()
        .flatten()
        .filter(|sensitive| sensitive.len() >= 4)
        .any(|sensitive| value.contains(sensitive))
    {
        let extension = value.rsplit_once('.').and_then(|(_, extension)| {
            (!extension.is_empty()
                && extension.len() <= 8
                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric()))
            .then_some(extension)
        });
        return extension.map_or_else(
            || "import".to_string(),
            |extension| format!("import.{extension}"),
        );
    }
    redact_file_name(value)
}

pub(super) fn mask_email(value: &str) -> String {
    let value = value.trim();
    let Some((local, domain)) = value.split_once('@') else {
        return mask_identifier(value);
    };
    let local = local.chars().next().unwrap_or('*');
    let (domain_name, suffix) = domain.rsplit_once('.').unwrap_or((domain, ""));
    let domain = domain_name.chars().next().unwrap_or('*');
    if suffix.is_empty() {
        format!("{local}***@{domain}***")
    } else {
        format!("{local}***@{domain}***.{suffix}")
    }
}

pub(super) fn mask_identifier(value: &str) -> String {
    let value = value.trim();
    if value.chars().count() <= 8 {
        return "****".to_string();
    }
    let prefix = value.chars().take(4).collect::<String>();
    let suffix = value
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{prefix}...{suffix}")
}

pub(super) fn sha256_hex(seed: &str, secret: Option<&str>, scope: Option<&str>) -> String {
    let mut digest = Sha256::new();
    digest.update(seed.as_bytes());
    if let Some(scope) = scope {
        digest.update([0]);
        digest.update(scope.as_bytes());
    }
    if let Some(secret) = secret {
        digest.update([0]);
        digest.update(secret.as_bytes());
    }
    hex::encode(digest.finalize())
}

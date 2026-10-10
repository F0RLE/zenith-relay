use super::*;
use crate::accounts::{MAX_ACCOUNT_TAGS, MAX_ACCOUNT_TAG_BYTES, MAX_ACCOUNT_TAG_CHARS};
use crate::{is_http_endpoint, url_has_userinfo};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use url::Url;

type ImportFieldLookup<'a> = (Option<&'a Map<String, Value>>, &'a [&'a str]);

pub(in crate::accounts::import) fn credential_string(
    import_object: &Map<String, Value>,
    credentials: &Map<String, Value>,
    tokens: Option<&Map<String, Value>>,
    fields: &[&str],
) -> Option<String> {
    credential_str(import_object, credentials, tokens, fields)
        .map(str::trim)
        .filter(|credential_text| !credential_text.is_empty())
        .map(str::to_string)
}

pub(in crate::accounts::import) fn credential_str<'a>(
    import_object: &'a Map<String, Value>,
    credentials: &'a Map<String, Value>,
    tokens: Option<&'a Map<String, Value>>,
    fields: &[&str],
) -> Option<&'a str> {
    tokens
        .and_then(|tokens| string_field(tokens, fields))
        .or_else(|| string_field(credentials, fields))
        .or_else(|| string_field(import_object, fields))
}

pub(in crate::accounts::import) fn credential_value<'a>(
    import_object: &'a Map<String, Value>,
    credentials: &'a Map<String, Value>,
    tokens: Option<&'a Map<String, Value>>,
    fields: &[&str],
) -> Option<&'a Value> {
    tokens
        .and_then(|tokens| value_field(tokens, fields))
        .or_else(|| value_field(credentials, fields))
        .or_else(|| value_field(import_object, fields))
}

pub(in crate::accounts::import) fn credential_bool(
    import_object: &Map<String, Value>,
    credentials: &Map<String, Value>,
    fields: &[&str],
) -> Option<bool> {
    value_field(credentials, fields)
        .or_else(|| value_field(import_object, fields))
        .and_then(Value::as_bool)
}

pub(in crate::accounts::import) fn string_field<'a>(
    json_object: &'a Map<String, Value>,
    fields: &[&str],
) -> Option<&'a str> {
    value_field(json_object, fields)?.as_str()
}

pub(in crate::accounts::import) fn first_string_field<'a>(
    lookups: &[ImportFieldLookup<'a>],
) -> Option<&'a str> {
    lookups.iter().find_map(|(lookup_object, fields)| {
        lookup_object.and_then(|json_object| string_field(json_object, fields))
    })
}

pub(in crate::accounts::import) fn first_value_field<'a>(
    lookups: &[ImportFieldLookup<'a>],
) -> Option<&'a Value> {
    lookups.iter().find_map(|(lookup_object, fields)| {
        lookup_object.and_then(|json_object| value_field(json_object, fields))
    })
}

pub(in crate::accounts::import) fn value_field<'a>(
    json_object: &'a Map<String, Value>,
    fields: &[&str],
) -> Option<&'a Value> {
    fields.iter().find_map(|field| json_object.get(*field))
}

pub(in crate::accounts::import) fn safe_identifier(
    identifier_value: Option<&str>,
) -> Option<String> {
    let identifier = identifier_value?.trim();
    if identifier.is_empty()
        || identifier.len() > 256
        || !identifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
    {
        None
    } else {
        Some(identifier.to_string())
    }
}

pub(in crate::accounts::import) fn safe_metadata(metadata_value: Option<&str>) -> Option<String> {
    let metadata_text = metadata_value?.trim();
    if metadata_text.is_empty()
        || metadata_text.len() > 64
        || !metadata_text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b' '))
    {
        None
    } else {
        Some(metadata_text.to_string())
    }
}

pub(in crate::accounts::import) fn safe_expiry(expiry_value: Option<&Value>) -> Option<String> {
    match expiry_value? {
        Value::Number(expiry_number) => Some(expiry_number.to_string()),
        Value::String(expiry_text)
            if !expiry_text.is_empty()
                && expiry_text.len() <= 64
                && expiry_text.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b':' | b'+' | b'.' | b' ')
                }) =>
        {
            Some(expiry_text.to_string())
        }
        _ => None,
    }
}

pub(in crate::accounts::import) fn safe_base_url(base_url_value: Option<&str>) -> Option<String> {
    let base_url_text = base_url_value?.trim();
    if base_url_text.is_empty() || base_url_text.len() > 2048 {
        return None;
    }
    let mut parsed = Url::parse(base_url_text).ok()?;
    if !is_http_endpoint(&parsed) || url_has_userinfo(&parsed) {
        return None;
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    Some(parsed.to_string().trim_end_matches('/').to_string())
}

pub(in crate::accounts::import) fn safe_protocol(protocol_value: Option<&str>) -> Option<String> {
    match protocol_value?.trim().to_ascii_lowercase().as_str() {
        "responses" => Some("responses".to_string()),
        "chat_completions" | "chat-completions" | "chat" => Some("chat_completions".to_string()),
        _ => None,
    }
}

pub(in crate::accounts::import) fn metadata_was_rejected(
    base_url_value: Option<&Value>,
    base_url: Option<&str>,
    protocol_value: Option<&Value>,
    protocol: Option<&str>,
    plan_value: Option<&Value>,
    plan: Option<&str>,
) -> bool {
    (base_url_value.is_some() && base_url.is_none())
        || (protocol_value.is_some() && protocol.is_none())
        || (plan_value.is_some() && plan.is_none())
}

/// Reads optional tags from a portable account item without allowing tags to
/// become a secret or an unbounded prepared-snapshot payload. Invalid entries
/// are ignored while valid entries remain importable; the boolean tells the
/// caller whether a metadata warning should be shown in the preview.
pub(in crate::accounts::import) fn safe_import_tags(
    tags_value: Option<&Value>,
    sensitive_values: &[Option<&str>],
) -> (BTreeSet<String>, bool) {
    let Some(tags_value) = tags_value else {
        return (BTreeSet::new(), false);
    };
    let Some(tag_values) = tags_value.as_array() else {
        return (BTreeSet::new(), true);
    };

    let mut tags = BTreeSet::new();
    let mut total_bytes = 0usize;
    let mut rejected = tag_values.len() > MAX_ACCOUNT_TAGS;
    for raw_tag in tag_values.iter().take(MAX_ACCOUNT_TAGS) {
        let Some(raw_tag) = raw_tag.as_str() else {
            rejected = true;
            continue;
        };
        let tag = raw_tag.trim();
        if tag.is_empty()
            || tag.chars().count() > MAX_ACCOUNT_TAG_CHARS
            || tag.chars().any(char::is_control)
            || sensitive_values
                .iter()
                .flatten()
                .filter(|sensitive| sensitive.len() >= 4)
                .any(|sensitive| tag.contains(sensitive))
        {
            rejected = true;
            continue;
        }
        if tags.contains(tag) {
            continue;
        }
        if total_bytes.saturating_add(tag.len()) > MAX_ACCOUNT_TAG_BYTES {
            rejected = true;
            continue;
        }
        total_bytes = total_bytes.saturating_add(tag.len());
        tags.insert(tag.to_string());
    }
    (tags, rejected)
}

pub(in crate::accounts::import) fn safe_label(label_value: Option<&str>) -> Option<String> {
    let label_text = label_value?.trim();
    if label_text.is_empty()
        || label_text.chars().count() > 80
        || label_text.chars().any(char::is_control)
    {
        return None;
    }
    Some(if label_text.contains('@') {
        mask_email(label_text)
    } else {
        label_text.to_string()
    })
}

pub(in crate::accounts::import) fn redact_label_secrets<'a>(
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

pub(in crate::accounts::import) fn redact_optional_metadata(
    metadata: &mut Option<String>,
    sensitive_values: &[Option<&str>],
) {
    if metadata.as_ref().is_some_and(|metadata| {
        sensitive_values
            .iter()
            .flatten()
            .filter(|sensitive| sensitive.len() >= 4)
            .any(|sensitive| metadata.contains(sensitive))
    }) {
        *metadata = None;
    }
}

pub(in crate::accounts::import) fn redact_file_name(file_name: &str) -> String {
    let (stem, extension) = file_name
        .rsplit_once('.')
        .filter(|(_, extension)| {
            !extension.is_empty()
                && extension.len() <= 8
                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .map_or((file_name, None), |(stem, extension)| {
            (stem, Some(extension))
        });
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

pub(in crate::accounts::import) fn redact_file_name_with(
    file_name: &str,
    sensitive_values: &[Option<&str>],
) -> String {
    if sensitive_values
        .iter()
        .flatten()
        .filter(|sensitive| sensitive.len() >= 4)
        .any(|sensitive| file_name.contains(sensitive))
    {
        let extension = file_name.rsplit_once('.').and_then(|(_, extension)| {
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
    redact_file_name(file_name)
}

pub(in crate::accounts::import) fn mask_email(email_value: &str) -> String {
    let email = email_value.trim();
    let Some((local, domain)) = email.split_once('@') else {
        return mask_identifier(email);
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

pub(in crate::accounts::import) fn mask_identifier(identifier_value: &str) -> String {
    let identifier = identifier_value.trim();
    if identifier.chars().count() <= 8 {
        return "****".to_string();
    }
    let prefix = identifier.chars().take(4).collect::<String>();
    let suffix = identifier
        .chars()
        .rev()
        .take(4)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{prefix}...{suffix}")
}

pub(in crate::accounts::import) fn sha256_hex(
    seed: &str,
    secret: Option<&str>,
    scope: Option<&str>,
) -> String {
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

use super::super::*;
use super::{InputEntry, ParsedEntries};

pub(super) fn is_zenith_bundle(object: &Map<String, Value>) -> bool {
    object
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|format| format.eq_ignore_ascii_case("zenith"))
}

pub(super) fn parse_zenith_bundle(
    object: &Map<String, Value>,
) -> Result<ParsedEntries, ImportError> {
    let version = object
        .get("version")
        .and_then(bundle_version)
        .ok_or_else(|| {
            ImportError::new(
                ImportErrorCode::MalformedJson,
                "Zenith account bundle version is missing",
            )
        })?;
    if version != 1 {
        return Err(ImportError::new(
            ImportErrorCode::UnsupportedBundleVersion,
            "Zenith account bundle version is unsupported",
        ));
    }
    let accounts = object
        .get("accounts")
        .and_then(Value::as_array)
        .filter(|accounts| !accounts.is_empty())
        .ok_or_else(|| {
            ImportError::new(
                ImportErrorCode::MalformedJson,
                "Zenith account bundle has no account list",
            )
        })?;
    check_item_count(accounts.len())?;
    let description = match object.get("description") {
        None | Some(Value::Null) => None,
        Some(Value::String(description)) => normalize_account_export_description(Some(description))
            .map_err(|_| {
                ImportError::new(
                    ImportErrorCode::MalformedJson,
                    "Zenith account bundle description is invalid",
                )
            })?
            .map(str::to_string),
        Some(_) => {
            return Err(ImportError::new(
                ImportErrorCode::MalformedJson,
                "Zenith account bundle description is invalid",
            ));
        }
    };
    Ok((
        ImportFormat::ZenithV1,
        account_entries(accounts),
        Vec::new(),
        description,
    ))
}

pub(super) fn is_portable_bundle(object: &Map<String, Value>) -> bool {
    let Some(accounts) = object.get("accounts").and_then(Value::as_array) else {
        return false;
    };
    object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.eq_ignore_ascii_case("portable_account_bundle"))
        || accounts.iter().any(|account| {
            account
                .as_object()
                .is_some_and(|account| account.get("credentials").is_some_and(Value::is_object))
        })
}

pub(super) fn wrapped_sub2api_payload(object: &Map<String, Value>) -> Option<&Map<String, Value>> {
    let payload = object.get("data").and_then(Value::as_object)?;
    let recognized = payload
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(is_sub2api_bundle_type);
    (recognized && payload.get("accounts").is_some_and(Value::is_array)).then_some(payload)
}

pub(super) fn parse_portable_bundle(
    object: &Map<String, Value>,
) -> Result<ParsedEntries, ImportError> {
    let version = object.get("version").and_then(bundle_version).unwrap_or(1);
    if version != 1 {
        return Err(ImportError::new(
            ImportErrorCode::UnsupportedBundleVersion,
            "portable account bundle version is unsupported",
        ));
    }
    let accounts = object
        .get("accounts")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ImportError::new(
                ImportErrorCode::MalformedJson,
                "portable account bundle has no account list",
            )
        })?;
    let (entries, warnings) = account_container(object, accounts)?;
    Ok((
        ImportFormat::PortableAccountBundleV1,
        entries,
        warnings,
        None,
    ))
}

pub(super) fn parse_account_container(
    object: &Map<String, Value>,
) -> Result<ParsedEntries, ImportError> {
    let accounts = object
        .get("accounts")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ImportError::new(
                ImportErrorCode::MalformedJson,
                "account container has no account list",
            )
        })?;
    let (entries, warnings) = account_container(object, accounts)?;
    Ok((ImportFormat::JsonArray, entries, warnings, None))
}

fn is_sub2api_bundle_type(kind: &str) -> bool {
    kind.eq_ignore_ascii_case("sub2api-data") || kind.eq_ignore_ascii_case("sub2api-bundle")
}

fn account_container(
    object: &Map<String, Value>,
    accounts: &[Value],
) -> Result<(Vec<InputEntry>, Vec<ImportWarning>), ImportError> {
    check_item_count(accounts.len())?;
    let proxy_count = object.get("proxies").map(container_count).unwrap_or(0);
    let warnings = (proxy_count > 0)
        .then(|| ImportWarning::count(ImportWarningCode::ProxiesIgnored, proxy_count))
        .into_iter()
        .collect();
    Ok((account_entries(accounts), warnings))
}

fn account_entries(accounts: &[Value]) -> Vec<InputEntry> {
    accounts
        .iter()
        .cloned()
        .enumerate()
        .map(|(ordinal, value)| InputEntry {
            ordinal,
            value: Some(value),
            issue: None,
        })
        .collect()
}

fn bundle_version(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.trim().parse().ok())
}

fn container_count(value: &Value) -> usize {
    match value {
        Value::Array(values) => values.len(),
        Value::Object(values) => values.len(),
        Value::Null => 0,
        _ => 1,
    }
}

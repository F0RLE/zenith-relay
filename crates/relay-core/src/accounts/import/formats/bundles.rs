use super::super::*;
use super::{InputEntry, ParsedEntries};

pub(super) fn is_zenith_bundle(bundle_object: &Map<String, Value>) -> bool {
    bundle_object
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|format| format.eq_ignore_ascii_case("zenith"))
}

pub(super) fn parse_zenith_bundle(
    bundle_object: &Map<String, Value>,
) -> Result<ParsedEntries, ImportError> {
    let version = bundle_object
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
    let accounts = bundle_object
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
    let description = match bundle_object.get("description") {
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

pub(super) fn is_portable_bundle(bundle_object: &Map<String, Value>) -> bool {
    let Some(accounts) = bundle_object.get("accounts").and_then(Value::as_array) else {
        return false;
    };
    bundle_object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind.eq_ignore_ascii_case("portable_account_bundle"))
        || accounts.iter().any(|account| {
            account
                .as_object()
                .is_some_and(|account| account.get("credentials").is_some_and(Value::is_object))
        })
}

pub(super) fn wrapped_sub2api_payload(
    bundle_object: &Map<String, Value>,
) -> Option<&Map<String, Value>> {
    let wrapped_bundle_object = bundle_object.get("data").and_then(Value::as_object)?;
    let recognized = wrapped_bundle_object
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(is_sub2api_bundle_type);
    (recognized
        && wrapped_bundle_object
            .get("accounts")
            .is_some_and(Value::is_array))
    .then_some(wrapped_bundle_object)
}

pub(super) fn parse_portable_bundle(
    bundle_object: &Map<String, Value>,
) -> Result<ParsedEntries, ImportError> {
    let version = bundle_object
        .get("version")
        .and_then(bundle_version)
        .unwrap_or(1);
    if version != 1 {
        return Err(ImportError::new(
            ImportErrorCode::UnsupportedBundleVersion,
            "portable account bundle version is unsupported",
        ));
    }
    let accounts = bundle_object
        .get("accounts")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ImportError::new(
                ImportErrorCode::MalformedJson,
                "portable account bundle has no account list",
            )
        })?;
    let (entries, warnings) = account_container(bundle_object, accounts)?;
    Ok((
        ImportFormat::PortableAccountBundleV1,
        entries,
        warnings,
        None,
    ))
}

pub(super) fn parse_account_container(
    bundle_object: &Map<String, Value>,
) -> Result<ParsedEntries, ImportError> {
    let accounts = bundle_object
        .get("accounts")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ImportError::new(
                ImportErrorCode::MalformedJson,
                "account container has no account list",
            )
        })?;
    let (entries, warnings) = account_container(bundle_object, accounts)?;
    Ok((ImportFormat::JsonArray, entries, warnings, None))
}

fn is_sub2api_bundle_type(kind: &str) -> bool {
    kind.eq_ignore_ascii_case("sub2api-data") || kind.eq_ignore_ascii_case("sub2api-bundle")
}

fn account_container(
    bundle_object: &Map<String, Value>,
    accounts: &[Value],
) -> Result<(Vec<InputEntry>, Vec<ImportWarning>), ImportError> {
    check_item_count(accounts.len())?;
    let proxy_count = bundle_object
        .get("proxies")
        .map(container_count)
        .unwrap_or(0);
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
        .map(|(ordinal, account_value)| InputEntry {
            ordinal,
            import_value: Some(account_value),
            issue: None,
        })
        .collect()
}

fn bundle_version(version_value: &Value) -> Option<u64> {
    version_value
        .as_u64()
        .or_else(|| version_value.as_str()?.trim().parse().ok())
}

fn container_count(container_value: &Value) -> usize {
    match container_value {
        Value::Array(container_values) => container_values.len(),
        Value::Object(container_values) => container_values.len(),
        Value::Null => 0,
        _ => 1,
    }
}

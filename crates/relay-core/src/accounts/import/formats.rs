use super::*;

pub(super) struct InputEntry {
    pub(super) ordinal: usize,
    pub(super) value: Option<Value>,
    pub(super) issue: Option<ImportIssue>,
}

pub(super) type ParsedEntries = (
    ImportFormat,
    Vec<InputEntry>,
    Vec<ImportWarning>,
    Option<String>,
);

pub(super) fn parse_entries(input: &str) -> Result<ParsedEntries, ImportError> {
    match serde_json::from_str::<Value>(input) {
        Ok(value) => {
            ensure_depth(&value)?;
            if let Some(object) = value.as_object() {
                if is_zenith_bundle(object) {
                    return parse_zenith_bundle(object);
                }
                if is_portable_bundle(object) {
                    return parse_portable_bundle(object);
                }
                if object.get("accounts").is_some_and(Value::is_array) {
                    return parse_account_container(object);
                }
                return Ok((
                    ImportFormat::JsonObject,
                    vec![InputEntry {
                        ordinal: 0,
                        value: Some(value),
                        issue: None,
                    }],
                    Vec::new(),
                    None,
                ));
            }
            if let Some(values) = value.as_array() {
                check_item_count(values.len())?;
                let entries = values
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(ordinal, value)| InputEntry {
                        ordinal,
                        value: Some(normalize_token_value(value)),
                        issue: None,
                    })
                    .collect();
                return Ok((ImportFormat::JsonArray, entries, Vec::new(), None));
            }
            Ok((
                ImportFormat::JsonObject,
                vec![InputEntry {
                    ordinal: 0,
                    value: Some(normalize_token_value(value)),
                    issue: None,
                }],
                Vec::new(),
                None,
            ))
        }
        Err(_) => parse_json_lines(input),
    }
}

fn parse_json_lines(input: &str) -> Result<ParsedEntries, ImportError> {
    let lines = input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return Err(ImportError::new(
            ImportErrorCode::EmptyInput,
            "import content is empty",
        ));
    }
    check_item_count(lines.len())?;

    let multiple = lines.len() > 1;
    let mut entries = Vec::with_capacity(lines.len());
    for (ordinal, line) in lines.into_iter().enumerate() {
        match serde_json::from_str::<Value>(line) {
            Ok(value) => {
                ensure_depth(&value)?;
                entries.push(InputEntry {
                    ordinal,
                    value: Some(normalize_token_value(value)),
                    issue: None,
                });
            }
            Err(_) => match raw_access_token(line) {
                Some(token) => entries.push(InputEntry {
                    ordinal,
                    value: Some(access_token_value(token)),
                    issue: None,
                }),
                None if multiple => entries.push(InputEntry {
                    ordinal,
                    value: None,
                    issue: Some(ImportIssue::new(
                        ImportIssueCode::MalformedJson,
                        "malformed JSON or access token line",
                    )),
                }),
                None => {
                    return Err(ImportError::new(
                        ImportErrorCode::MalformedJson,
                        "import content is not valid JSON or an access token",
                    ));
                }
            },
        }
    }
    Ok((ImportFormat::JsonLines, entries, Vec::new(), None))
}

fn normalize_token_value(value: Value) -> Value {
    match value {
        Value::String(value) => raw_access_token(&value)
            .map(access_token_value)
            .unwrap_or(Value::String(value)),
        value => value,
    }
}

fn raw_access_token(value: &str) -> Option<&str> {
    let value = value.trim();
    let token = value
        .get(..7)
        .filter(|prefix| prefix.eq_ignore_ascii_case("bearer "))
        .and_then(|_| value.get(7..))
        .map(str::trim)
        .unwrap_or(value);
    if token.is_empty()
        || token.len() > MAX_RAW_TOKEN_BYTES
        || !token.is_ascii()
        || token
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
    {
        return None;
    }
    let mut parts = token.split('.');
    let jwt = matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some(header), Some(payload), Some(signature), None)
            if !header.is_empty() && !payload.is_empty() && !signature.is_empty()
    );
    (jwt || token
        .strip_prefix("at-")
        .is_some_and(|value| !value.is_empty()))
    .then_some(token)
}

fn access_token_value(token: &str) -> Value {
    serde_json::json!({ "access_token": token })
}

pub(super) fn malformed_import_value() -> Value {
    serde_json::json!({ IMPORT_ERROR_MARKER: true })
}

fn is_zenith_bundle(object: &Map<String, Value>) -> bool {
    object
        .get("format")
        .and_then(Value::as_str)
        .is_some_and(|format| format.eq_ignore_ascii_case("zenith"))
}

fn parse_zenith_bundle(object: &Map<String, Value>) -> Result<ParsedEntries, ImportError> {
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
    let entries = accounts
        .iter()
        .cloned()
        .enumerate()
        .map(|(ordinal, value)| InputEntry {
            ordinal,
            value: Some(value),
            issue: None,
        })
        .collect();
    Ok((ImportFormat::ZenithV1, entries, Vec::new(), description))
}

fn is_portable_bundle(object: &Map<String, Value>) -> bool {
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

fn parse_portable_bundle(object: &Map<String, Value>) -> Result<ParsedEntries, ImportError> {
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
    let (entries, warnings) = parse_account_container_items(object, accounts)?;
    Ok((
        ImportFormat::PortableAccountBundleV1,
        entries,
        warnings,
        None,
    ))
}

fn parse_account_container(object: &Map<String, Value>) -> Result<ParsedEntries, ImportError> {
    let accounts = object
        .get("accounts")
        .and_then(Value::as_array)
        .expect("account container checked by caller");
    let (entries, warnings) = parse_account_container_items(object, accounts)?;
    Ok((ImportFormat::JsonArray, entries, warnings, None))
}

fn parse_account_container_items(
    object: &Map<String, Value>,
    accounts: &[Value],
) -> Result<(Vec<InputEntry>, Vec<ImportWarning>), ImportError> {
    check_item_count(accounts.len())?;
    let entries = accounts
        .iter()
        .cloned()
        .enumerate()
        .map(|(ordinal, value)| InputEntry {
            ordinal,
            value: Some(value),
            issue: None,
        })
        .collect();
    let proxy_count = object.get("proxies").map(container_count).unwrap_or(0);
    let warnings = (proxy_count > 0)
        .then(|| ImportWarning::count(ImportWarningCode::ProxiesIgnored, proxy_count))
        .into_iter()
        .collect();
    Ok((entries, warnings))
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

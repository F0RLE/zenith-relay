use super::super::sanitization::*;
use super::super::*;
use super::vocabulary::*;
use crate::providers::chatgpt::{
    BasisPointsCapturedHeaders, BasisPointsHeader, BasisPointsHeadersError,
};

fn basis_points_headers_from_import(
    import_object: &Map<String, Value>,
    credentials: &Map<String, Value>,
) -> Result<Option<BasisPointsCapturedHeaders>, ImportIssue> {
    let mut entries: Vec<BasisPointsHeader> = Vec::new();
    for value in [
        import_object.get("headers"),
        credentials.get("headers"),
        import_object.get("captured_headers"),
        credentials.get("captured_headers"),
    ] {
        let Some(value) = value else {
            continue;
        };
        let parsed =
            BasisPointsCapturedHeaders::from_json_values(Some(value), None).map_err(|error| {
                let message = match error {
                    BasisPointsHeadersError::InvalidName => {
                        "imported Basis Points header name is invalid"
                    }
                    BasisPointsHeadersError::InvalidValue => {
                        "imported Basis Points header value is invalid"
                    }
                    BasisPointsHeadersError::TooManyHeaders => {
                        "imported Basis Points headers contain too many entries"
                    }
                    BasisPointsHeadersError::TooLarge => {
                        "imported Basis Points headers are too large"
                    }
                };
                ImportIssue::new(ImportIssueCode::InvalidCredentials, message)
            })?;
        entries.extend(parsed.entries().iter().cloned());
    }
    for container in [import_object, credentials] {
        let Some(value) = container
            .get("basis_points_headers")
            .or_else(|| container.get("basisPointsHeaders"))
        else {
            continue;
        };
        let headers: BasisPointsCapturedHeaders =
            serde_json::from_value(value.clone()).map_err(|_| {
                ImportIssue::new(
                    ImportIssueCode::InvalidCredentials,
                    "imported Basis Points headers are invalid",
                )
            })?;
        headers.validate().map_err(|_| {
            ImportIssue::new(
                ImportIssueCode::InvalidCredentials,
                "imported Basis Points headers are invalid",
            )
        })?;
        entries.extend(headers.entries().iter().cloned());
    }
    let parsed = BasisPointsCapturedHeaders::from_entries(entries).map_err(|error| {
        let message = match error {
            BasisPointsHeadersError::InvalidName => "imported Basis Points header name is invalid",
            BasisPointsHeadersError::InvalidValue => {
                "imported Basis Points header value is invalid"
            }
            BasisPointsHeadersError::TooManyHeaders => {
                "imported Basis Points headers contain too many entries"
            }
            BasisPointsHeadersError::TooLarge => "imported Basis Points headers are too large",
        };
        ImportIssue::new(ImportIssueCode::InvalidCredentials, message)
    })?;
    Ok((!parsed.is_empty()).then_some(parsed))
}

fn bearer_access_token_from_headers(
    import_object: &Map<String, Value>,
    credentials: &Map<String, Value>,
) -> Option<String> {
    [credentials, import_object]
        .into_iter()
        .find_map(|container| {
            container
                .get("headers")
                .and_then(Value::as_object)
                .and_then(|headers| string_field(headers, &["authorization", "Authorization"]))
                .and_then(bearer_access_token)
        })
}

fn oauth_client_kind_from_import(
    import_object: &Map<String, Value>,
    credentials: &Map<String, Value>,
    tokens: Option<&Map<String, Value>>,
    id_token: Option<&str>,
    access_token: Option<&str>,
) -> Result<Option<OAuthClientKind>, ImportIssue> {
    let mut containers = vec![import_object, credentials];
    if let Some(tokens) = tokens {
        containers.push(tokens);
    }
    let mut selected = None;
    for container in containers {
        for source in std::iter::once(container).chain(
            ["oauth", "oauth_client", "oauthClient"]
                .into_iter()
                .filter_map(|field| container.get(field).and_then(Value::as_object)),
        ) {
            for field in [
                "client_id",
                "clientId",
                "oauth_client_id",
                "oauthClientId",
                "oauth_client_kind",
                "oauthClientKind",
                "oauth_client",
            ] {
                let Some(value) = source.get(field) else {
                    continue;
                };
                if field == "oauth_client" && value.is_object() {
                    continue;
                }
                let kind = value
                    .as_str()
                    .and_then(|text| {
                        if matches!(
                            field,
                            "oauth_client_kind" | "oauthClientKind" | "oauth_client"
                        ) {
                            match text.trim() {
                                "codex" => Some(OAuthClientKind::Codex),
                                "excel" | "excel_bps" => Some(OAuthClientKind::ExcelBps),
                                _ => None,
                            }
                        } else {
                            OAuthClientKind::from_client_id(text.trim())
                        }
                    })
                    .ok_or_else(|| {
                        ImportIssue::new(
                            ImportIssueCode::UnsupportedValue,
                            "imported OAuth client is unsupported",
                        )
                    })?;
                merge_oauth_client_hint(&mut selected, kind)?;
            }
        }
    }
    let token_hint = OAuthClientKind::from_token_hints(id_token, access_token)
        .map_err(|message| ImportIssue::new(ImportIssueCode::AmbiguousCredentials, message))?;
    if let Some(kind) = token_hint {
        merge_oauth_client_hint(&mut selected, kind)?;
    }
    Ok(selected)
}

fn merge_oauth_client_hint(
    selected: &mut Option<OAuthClientKind>,
    kind: OAuthClientKind,
) -> Result<(), ImportIssue> {
    if selected.is_some_and(|selected| selected != kind) {
        return Err(ImportIssue::new(
            ImportIssueCode::AmbiguousCredentials,
            "imported OAuth client metadata conflicts",
        ));
    }
    *selected = Some(kind);
    Ok(())
}

fn chatgpt_account_id_header<'a>(
    import_object: &'a Map<String, Value>,
    credentials: &'a Map<String, Value>,
) -> Option<&'a str> {
    [import_object, credentials]
        .into_iter()
        .find_map(|container| {
            ["headers", "custom_headers", "customHeaders"]
                .into_iter()
                .find_map(|header_container_name| {
                    container
                        .get(header_container_name)
                        .and_then(Value::as_object)
                        .and_then(chatgpt_account_header_value)
                })
        })
}

fn chatgpt_account_header_value(headers: &Map<String, Value>) -> Option<&str> {
    headers.iter().find_map(|(header_name, header_value)| {
        header_name
            .eq_ignore_ascii_case("chatgpt-account-id")
            .then(|| header_value.as_str())
            .flatten()
            .map(str::trim)
            .filter(|account_id| !account_id.is_empty())
    })
}

fn bearer_access_token(authorization_value: &str) -> Option<String> {
    let trimmed_authorization = authorization_value.trim();
    let token = trimmed_authorization
        .get(..7)
        .filter(|prefix| prefix.eq_ignore_ascii_case("bearer "))
        .and_then(|_| trimmed_authorization.get(7..))?
        .trim();
    (!token.is_empty()
        && token.len() <= MAX_RAW_TOKEN_BYTES
        && token.is_ascii()
        && !token
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control()))
    .then(|| token.to_string())
}

pub(super) struct PresentedImport<'a> {
    pub(super) import_object: &'a Map<String, Value>,
    pub(super) credentials: &'a Map<String, Value>,
    pub(super) account: Option<&'a Map<String, Value>>,
    pub(super) identity: Option<&'a Map<String, Value>>,
    pub(super) subscription: Option<&'a Map<String, Value>>,
    pub(super) user: Option<&'a Map<String, Value>>,
    pub(super) session_profile: Option<&'a Map<String, Value>>,
    pub(super) header_account_id: Option<&'a str>,
    pub(super) agent_identity_object: Option<&'a Map<String, Value>>,
    pub(super) provider_object: Option<&'a Map<String, Value>>,
    pub(super) meta: Option<&'a Map<String, Value>>,
    pub(super) tags_value: Option<&'a Value>,
    pub(super) basis_points_headers: Option<BasisPointsCapturedHeaders>,
    pub(super) oauth_client_kind: Option<OAuthClientKind>,
    pub(super) api_key: Option<String>,
    pub(super) access_token: Option<String>,
    pub(super) refresh_token: Option<String>,
    pub(super) id_token: Option<String>,
    pub(super) agent_private_key: Option<String>,
    pub(super) agent_runtime_id: Option<String>,
    pub(super) agent_task_id: Option<String>,
    pub(super) explicit_auth_mode: Option<&'a str>,
}

pub(super) fn present_import_item<'a>(
    import_object: &'a Map<String, Value>,
    format: ImportFormat,
) -> Result<PresentedImport<'a>, ImportIssue> {
    let auth = import_object.get("auth").and_then(Value::as_object);
    let account = import_object.get("account").and_then(Value::as_object);
    let identity = import_object.get("identity").and_then(Value::as_object);
    let subscription = import_object.get("subscription").and_then(Value::as_object);
    let user = import_object.get("user").and_then(Value::as_object);
    let session_profile = import_object.get("profile").and_then(Value::as_object);
    let credentials = import_object
        .get("credentials")
        .and_then(Value::as_object)
        .or_else(|| (format == ImportFormat::ZenithV1).then_some(auth).flatten())
        .unwrap_or(import_object);
    if format == ImportFormat::PortableAccountBundleV1
        && string_field(import_object, &["platform"])
            .is_some_and(|platform| !is_openai_platform(platform))
    {
        return Err(ImportIssue::new(
            ImportIssueCode::UnsupportedValue,
            "portable import item is not a ChatGPT account",
        ));
    }
    // Cockpit and Sub2API can keep Agent Identity as a nested object instead
    // of flattening its fields into `credentials`.  Treat that object as a
    // credential source, but do not use arbitrary nested objects elsewhere.
    let agent_identity_object = credentials
        .get("agent_identity")
        .or_else(|| credentials.get("agentIdentity"))
        .and_then(Value::as_object)
        .or_else(|| {
            import_object
                .get("agent_identity")
                .or_else(|| import_object.get("agentIdentity"))
                .and_then(Value::as_object)
        });
    let provider_object = import_object
        .get("providerSpecificData")
        .or_else(|| import_object.get("provider_specific_data"))
        .and_then(Value::as_object);
    let metadata_object = import_object.get("meta").and_then(Value::as_object);
    // Cockpit writes tags at the item root. Accept the nested locations used
    // by portable/Sub2API exports as well, but keep the lookup explicit rather
    // than walking arbitrary JSON metadata.
    let tags_value = value_field(import_object, TAG_FIELDS)
        .or_else(|| credentials.get("tags"))
        .or_else(|| account.and_then(|account_object| value_field(account_object, TAG_FIELDS)))
        .or_else(|| {
            metadata_object.and_then(|metadata_object| value_field(metadata_object, TAG_FIELDS))
        })
        .or_else(|| {
            provider_object.and_then(|provider_object| value_field(provider_object, TAG_FIELDS))
        });
    let tokens = import_object
        .get("tokens")
        .and_then(Value::as_object)
        .or_else(|| credentials.get("tokens").and_then(Value::as_object));

    let api_key = credential_string(import_object, credentials, tokens, API_KEY_FIELDS);
    let access_token = credential_string(import_object, credentials, tokens, ACCESS_TOKEN_FIELDS)
        .or_else(|| bearer_access_token_from_headers(import_object, credentials));
    let refresh_token = credential_string(import_object, credentials, tokens, REFRESH_TOKEN_FIELDS)
        .filter(|refresh_token| refresh_token != "__missing_refresh_token__");
    let id_token = credential_string(import_object, credentials, tokens, ID_TOKEN_FIELDS);
    let header_account_id = chatgpt_account_id_header(import_object, credentials);
    let basis_points_headers = basis_points_headers_from_import(import_object, credentials)?;
    let oauth_client_kind = if access_token.is_some() || refresh_token.is_some() {
        oauth_client_kind_from_import(
            import_object,
            credentials,
            tokens,
            id_token.as_deref(),
            access_token.as_deref(),
        )?
    } else {
        None
    };
    let agent_private_key = agent_identity_object
        .and_then(|agent_identity_object| {
            string_field(agent_identity_object, AGENT_PRIVATE_KEY_FIELDS)
        })
        .map(str::to_string)
        .or_else(|| credential_string(import_object, credentials, None, AGENT_PRIVATE_KEY_FIELDS));
    let agent_runtime_id = agent_identity_object
        .and_then(|agent_identity_object| {
            string_field(agent_identity_object, AGENT_RUNTIME_ID_FIELDS)
        })
        .map(str::to_string)
        .or_else(|| credential_string(import_object, credentials, None, AGENT_RUNTIME_ID_FIELDS));
    let agent_task_id = agent_identity_object
        .and_then(|agent_identity_object| string_field(agent_identity_object, AGENT_TASK_ID_FIELDS))
        .map(str::to_string)
        .or_else(|| credential_string(import_object, credentials, None, AGENT_TASK_ID_FIELDS));
    if oauth_client_kind == Some(OAuthClientKind::ExcelBps)
        && (agent_private_key.is_some() || agent_runtime_id.is_some() || agent_task_id.is_some())
    {
        return Err(ImportIssue::new(
            ImportIssueCode::AmbiguousCredentials,
            "Excel OAuth cannot use Agent Identity",
        ));
    }
    let named_auth_mode = string_field(credentials, AUTH_MODE_FIELDS)
        .or_else(|| string_field(import_object, AUTH_MODE_FIELDS))
        .or_else(|| auth.and_then(|auth| string_field(auth, &["type"])));
    // Sub2API's versioned data export declares its account kind as the outer
    // `type` field (`oauth`, `apikey`, ...), while Cockpit uses `auth_mode`.
    // Only treat a generic `type` as an auth declaration when it is a known
    // credential kind: top-level values such as `codex` are file labels.
    let typed_auth_mode = string_field(credentials, &["type"])
        .or_else(|| string_field(import_object, &["type"]))
        .filter(|auth_mode| is_recognized_auth_mode(auth_mode));
    let explicit_auth_mode = named_auth_mode.or(typed_auth_mode);
    Ok(PresentedImport {
        import_object,
        credentials,
        account,
        identity,
        subscription,
        user,
        session_profile,
        header_account_id,
        agent_identity_object,
        provider_object,
        meta: metadata_object,
        tags_value,
        basis_points_headers,
        oauth_client_kind,
        api_key,
        access_token,
        refresh_token,
        id_token,
        agent_private_key,
        agent_runtime_id,
        agent_task_id,
        explicit_auth_mode,
    })
}

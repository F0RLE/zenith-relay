use super::super::sanitization::*;
use super::super::*;
use super::vocabulary::*;

fn bearer_access_token_from_headers(
    object: &Map<String, Value>,
    credentials: &Map<String, Value>,
) -> Option<String> {
    [credentials, object].into_iter().find_map(|container| {
        container
            .get("headers")
            .and_then(Value::as_object)
            .and_then(|headers| string_field(headers, &["authorization", "Authorization"]))
            .and_then(bearer_access_token)
    })
}

fn chatgpt_account_id_header<'a>(
    object: &'a Map<String, Value>,
    credentials: &'a Map<String, Value>,
) -> Option<&'a str> {
    [object, credentials].into_iter().find_map(|container| {
        ["headers", "custom_headers", "customHeaders"]
            .into_iter()
            .find_map(|name| {
                container
                    .get(name)
                    .and_then(Value::as_object)
                    .and_then(chatgpt_account_header_value)
            })
    })
}

fn chatgpt_account_header_value(headers: &Map<String, Value>) -> Option<&str> {
    headers.iter().find_map(|(key, value)| {
        key.eq_ignore_ascii_case("chatgpt-account-id")
            .then(|| value.as_str())
            .flatten()
            .map(str::trim)
            .filter(|value| !value.is_empty())
    })
}

fn bearer_access_token(value: &str) -> Option<String> {
    let value = value.trim();
    let token = value
        .get(..7)
        .filter(|prefix| prefix.eq_ignore_ascii_case("bearer "))
        .and_then(|_| value.get(7..))?
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
    pub(super) object: &'a Map<String, Value>,
    pub(super) credentials: &'a Map<String, Value>,
    pub(super) account: Option<&'a Map<String, Value>>,
    pub(super) identity: Option<&'a Map<String, Value>>,
    pub(super) subscription: Option<&'a Map<String, Value>>,
    pub(super) user: Option<&'a Map<String, Value>>,
    pub(super) session_profile: Option<&'a Map<String, Value>>,
    pub(super) header_account_id: Option<&'a str>,
    pub(super) agent_identity_data: Option<&'a Map<String, Value>>,
    pub(super) provider_data: Option<&'a Map<String, Value>>,
    pub(super) meta: Option<&'a Map<String, Value>>,
    pub(super) tags_value: Option<&'a Value>,
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
    object: &'a Map<String, Value>,
    format: ImportFormat,
) -> Result<PresentedImport<'a>, ImportIssue> {
    let auth = object.get("auth").and_then(Value::as_object);
    let account = object.get("account").and_then(Value::as_object);
    let identity = object.get("identity").and_then(Value::as_object);
    let subscription = object.get("subscription").and_then(Value::as_object);
    let user = object.get("user").and_then(Value::as_object);
    let session_profile = object.get("profile").and_then(Value::as_object);
    let credentials = object
        .get("credentials")
        .and_then(Value::as_object)
        .or_else(|| (format == ImportFormat::ZenithV1).then_some(auth).flatten())
        .unwrap_or(object);
    if format == ImportFormat::PortableAccountBundleV1
        && string_field(object, &["platform"]).is_some_and(|platform| !is_openai_platform(platform))
    {
        return Err(ImportIssue::new(
            ImportIssueCode::UnsupportedValue,
            "portable import item is not an OpenAI/Codex account",
        ));
    }
    // Cockpit and Sub2API can keep Agent Identity as a nested object instead
    // of flattening its fields into `credentials`.  Treat that object as a
    // credential source, but do not use arbitrary nested objects elsewhere.
    let agent_identity_data = credentials
        .get("agent_identity")
        .or_else(|| credentials.get("agentIdentity"))
        .and_then(Value::as_object)
        .or_else(|| {
            object
                .get("agent_identity")
                .or_else(|| object.get("agentIdentity"))
                .and_then(Value::as_object)
        });
    let provider_data = object
        .get("providerSpecificData")
        .or_else(|| object.get("provider_specific_data"))
        .and_then(Value::as_object);
    let meta = object.get("meta").and_then(Value::as_object);
    // Cockpit writes tags at the item root. Accept the nested locations used
    // by portable/Sub2API exports as well, but keep the lookup explicit rather
    // than walking arbitrary JSON metadata.
    let tags_value = value_field(object, TAG_FIELDS)
        .or_else(|| credentials.get("tags"))
        .or_else(|| account.and_then(|data| value_field(data, TAG_FIELDS)))
        .or_else(|| meta.and_then(|data| value_field(data, TAG_FIELDS)))
        .or_else(|| provider_data.and_then(|data| value_field(data, TAG_FIELDS)));
    let tokens = object
        .get("tokens")
        .and_then(Value::as_object)
        .or_else(|| credentials.get("tokens").and_then(Value::as_object));

    let api_key = credential_string(object, credentials, tokens, API_KEY_FIELDS);
    let access_token = credential_string(object, credentials, tokens, ACCESS_TOKEN_FIELDS)
        .or_else(|| bearer_access_token_from_headers(object, credentials));
    let refresh_token = credential_string(object, credentials, tokens, REFRESH_TOKEN_FIELDS)
        .filter(|value| value != "__missing_refresh_token__");
    let id_token = credential_string(object, credentials, tokens, ID_TOKEN_FIELDS);
    let header_account_id = chatgpt_account_id_header(object, credentials);
    let agent_private_key = agent_identity_data
        .and_then(|data| string_field(data, AGENT_PRIVATE_KEY_FIELDS))
        .map(str::to_string)
        .or_else(|| credential_string(object, credentials, None, AGENT_PRIVATE_KEY_FIELDS));
    let agent_runtime_id = agent_identity_data
        .and_then(|data| string_field(data, AGENT_RUNTIME_ID_FIELDS))
        .map(str::to_string)
        .or_else(|| credential_string(object, credentials, None, AGENT_RUNTIME_ID_FIELDS));
    let agent_task_id = agent_identity_data
        .and_then(|data| string_field(data, AGENT_TASK_ID_FIELDS))
        .map(str::to_string)
        .or_else(|| credential_string(object, credentials, None, AGENT_TASK_ID_FIELDS));
    let named_auth_mode = string_field(credentials, AUTH_MODE_FIELDS)
        .or_else(|| string_field(object, AUTH_MODE_FIELDS))
        .or_else(|| auth.and_then(|auth| string_field(auth, &["type"])));
    // Sub2API's versioned data export declares its account kind as the outer
    // `type` field (`oauth`, `apikey`, ...), while Cockpit uses `auth_mode`.
    // Only treat a generic `type` as an auth declaration when it is a known
    // credential kind: top-level values such as `codex` are file labels.
    let typed_auth_mode = string_field(credentials, &["type"])
        .or_else(|| string_field(object, &["type"]))
        .filter(|value| is_recognized_auth_mode(value));
    let explicit_auth_mode = named_auth_mode.or(typed_auth_mode);
    Ok(PresentedImport {
        object,
        credentials,
        account,
        identity,
        subscription,
        user,
        session_profile,
        header_account_id,
        agent_identity_data,
        provider_data,
        meta,
        tags_value,
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

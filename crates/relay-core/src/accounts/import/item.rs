use super::sanitization::*;
use super::*;

pub(super) fn parse_item(
    value: &Value,
    ordinal: usize,
    format: ImportFormat,
    source_file: Option<&str>,
) -> Result<ParsedItem, ImportIssue> {
    let object = value.as_object().ok_or_else(|| {
        ImportIssue::new(
            ImportIssueCode::UnsupportedValue,
            "import item must be a JSON object",
        )
    })?;
    if object
        .get(IMPORT_ERROR_MARKER)
        .is_some_and(Value::is_boolean)
    {
        return Err(ImportIssue::new(
            ImportIssueCode::MalformedJson,
            "import file or line is malformed",
        ));
    }
    let auth = object.get("auth").and_then(Value::as_object);
    let account = object.get("account").and_then(Value::as_object);
    let identity = object.get("identity").and_then(Value::as_object);
    let subscription = object.get("subscription").and_then(Value::as_object);
    let credentials = object
        .get("credentials")
        .and_then(Value::as_object)
        .or_else(|| (format == ImportFormat::ZenithV1).then_some(auth).flatten())
        .unwrap_or(object);
    let provider_data = object
        .get("providerSpecificData")
        .or_else(|| object.get("provider_specific_data"))
        .and_then(Value::as_object);
    let meta = object.get("meta").and_then(Value::as_object);
    let tokens = object
        .get("tokens")
        .and_then(Value::as_object)
        .or_else(|| credentials.get("tokens").and_then(Value::as_object));

    let api_key = credential_string(object, credentials, tokens, API_KEY_FIELDS);
    let access_token = credential_string(object, credentials, tokens, ACCESS_TOKEN_FIELDS);
    let refresh_token = credential_string(object, credentials, tokens, REFRESH_TOKEN_FIELDS)
        .filter(|value| value != "__missing_refresh_token__");
    let id_token = credential_string(object, credentials, tokens, ID_TOKEN_FIELDS);
    let agent_private_key = credential_string(object, credentials, None, AGENT_PRIVATE_KEY_FIELDS);
    let agent_runtime_id = credential_string(object, credentials, None, AGENT_RUNTIME_ID_FIELDS);
    let agent_task_id = credential_string(object, credentials, None, AGENT_TASK_ID_FIELDS);
    let has_api_key = api_key.is_some();
    let has_tokens = access_token.is_some() || refresh_token.is_some() || id_token.is_some();
    let has_agent_identity =
        agent_private_key.is_some() || agent_runtime_id.is_some() || agent_task_id.is_some();
    let explicit_auth_mode = string_field(credentials, &["auth_mode", "authMode", "authType"])
        .or_else(|| string_field(object, &["auth_mode", "authMode", "authType"]))
        .or_else(|| auth.and_then(|auth| string_field(auth, &["type"])));
    let explicit_oauth = explicit_auth_mode.is_some_and(is_oauth_mode);
    let explicit_agent_identity = explicit_auth_mode.is_some_and(is_agent_identity_mode);
    let explicit_api_key = explicit_auth_mode.is_some_and(is_api_key_mode);
    if has_agent_identity && (agent_private_key.is_none() || agent_runtime_id.is_none()) {
        return Err(ImportIssue::new(
            ImportIssueCode::InvalidCredentials,
            "Agent Identity import requires a private key and runtime id",
        ));
    }
    let mut warnings = Vec::new();
    let credential_kind_count =
        usize::from(has_api_key) + usize::from(has_tokens) + usize::from(has_agent_identity);
    let has_account_credentials = has_tokens || has_agent_identity;
    let (use_api_key, use_tokens, use_agent_identity) = if has_api_key && has_account_credentials {
        if explicit_api_key {
            warnings.push(ImportWarning::new(
                ImportWarningCode::UnusedCredentialsIgnored,
            ));
            (true, false, false)
        } else if explicit_oauth || explicit_agent_identity {
            warnings.push(ImportWarning::new(
                ImportWarningCode::UnusedCredentialsIgnored,
            ));
            (false, has_tokens, has_agent_identity)
        } else {
            return Err(ImportIssue::new(
                ImportIssueCode::AmbiguousCredentials,
                "import item mixes an API key with account credentials",
            ));
        }
    } else {
        (has_api_key, has_tokens, has_agent_identity)
    };
    if credential_kind_count == 0 {
        return Err(ImportIssue::new(
            ImportIssueCode::MissingCredentials,
            "import item has no supported credential",
        ));
    }
    if use_tokens && access_token.is_none() && refresh_token.is_none() {
        return Err(ImportIssue::new(
            ImportIssueCode::InvalidCredentials,
            "token import requires an access or refresh token",
        ));
    }

    let auth_mode = if use_api_key {
        ImportAuthMode::ApiKey
    } else if use_agent_identity {
        ImportAuthMode::AgentIdentity
    } else if explicit_oauth {
        ImportAuthMode::OAuth
    } else {
        if explicit_auth_mode.is_some_and(|mode| !is_token_mode(mode)) {
            warnings.push(ImportWarning::new(ImportWarningCode::UnknownAuthMode));
        }
        ImportAuthMode::ImportedToken
    };
    if use_tokens && access_token.is_some() && refresh_token.is_none() {
        warnings.push(ImportWarning::new(ImportWarningCode::AccessTokenOnly));
    }
    if use_tokens && access_token.is_none() && refresh_token.is_some() {
        warnings.push(ImportWarning::new(
            ImportWarningCode::RefreshExchangeRequired,
        ));
    }
    let jwt = imported_jwt_metadata(id_token.as_deref(), access_token.as_deref());
    if object.contains_key("concurrency") {
        warnings.push(ImportWarning::new(ImportWarningCode::ConcurrencyIgnored));
    }

    let email = credential_string(object, credentials, None, EMAIL_FIELDS)
        .or_else(|| {
            account
                .and_then(|data| string_field(data, EMAIL_FIELDS))
                .map(str::to_string)
        })
        .or_else(|| {
            provider_data
                .and_then(|data| string_field(data, EMAIL_FIELDS))
                .map(str::to_string)
        })
        .or_else(|| {
            identity
                .and_then(|data| string_field(data, EMAIL_FIELDS))
                .map(str::to_string)
        })
        .or(jwt.email);
    let account_id_value = credential_str(object, credentials, None, ACCOUNT_ID_FIELDS)
        .or_else(|| account.and_then(|data| string_field(data, &["id"])))
        .or_else(|| account.and_then(|data| string_field(data, ACCOUNT_ID_FIELDS)))
        .or_else(|| provider_data.and_then(|data| string_field(data, ACCOUNT_ID_FIELDS)))
        .or_else(|| meta.and_then(|data| string_field(data, ACCOUNT_ID_FIELDS)))
        .or_else(|| identity.and_then(|data| string_field(data, ACCOUNT_ID_FIELDS)));
    let account_id = safe_identifier(account_id_value).or(jwt.account_id);
    let chatgpt_user_id_value = credential_str(object, credentials, None, USER_ID_FIELDS)
        .or_else(|| account.and_then(|data| string_field(data, USER_ID_FIELDS)))
        .or_else(|| provider_data.and_then(|data| string_field(data, USER_ID_FIELDS)))
        .or_else(|| meta.and_then(|data| string_field(data, USER_ID_FIELDS)))
        .or_else(|| identity.and_then(|data| string_field(data, USER_ID_FIELDS)));
    let chatgpt_user_id = safe_identifier(chatgpt_user_id_value).or(jwt.user_id);
    let organization_id_value = credential_str(object, credentials, None, ORGANIZATION_ID_FIELDS)
        .or_else(|| account.and_then(|data| string_field(data, ORGANIZATION_ID_FIELDS)))
        .or_else(|| provider_data.and_then(|data| string_field(data, ORGANIZATION_ID_FIELDS)))
        .or_else(|| meta.and_then(|data| string_field(data, ORGANIZATION_ID_FIELDS)))
        .or_else(|| identity.and_then(|data| string_field(data, ORGANIZATION_ID_FIELDS)));
    let organization_id = safe_identifier(organization_id_value);
    let plan_value = credential_value(object, credentials, None, PLAN_FIELDS)
        .or_else(|| account.and_then(|data| value_field(data, PLAN_FIELDS)))
        .or_else(|| provider_data.and_then(|data| value_field(data, PLAN_FIELDS)))
        .or_else(|| meta.and_then(|data| value_field(data, PLAN_FIELDS)))
        .or_else(|| subscription.and_then(|data| value_field(data, PLAN_FIELDS)));
    let mut plan = safe_metadata(plan_value.and_then(Value::as_str)).or(jwt.plan_type);
    let expires_at_value = credential_value(object, credentials, None, EXPIRES_AT_FIELDS)
        .or_else(|| provider_data.and_then(|data| value_field(data, EXPIRES_AT_FIELDS)));
    let mut expires_at = safe_expiry(expires_at_value).or(jwt.expires_at);
    let subscription_expires_at_value =
        credential_value(object, credentials, None, SUBSCRIPTION_EXPIRES_AT_FIELDS)
            .or_else(|| account.and_then(|data| value_field(data, SUBSCRIPTION_EXPIRES_AT_FIELDS)))
            .or_else(|| {
                provider_data.and_then(|data| value_field(data, SUBSCRIPTION_EXPIRES_AT_FIELDS))
            })
            .or_else(|| {
                subscription.and_then(|data| value_field(data, &["expiresAt", "expires_at"]))
            });
    let mut subscription_expires_at =
        safe_expiry(subscription_expires_at_value).or(jwt.subscription_expires_at);
    let base_url_value = value_field(
        object,
        &[
            "base_url",
            "baseUrl",
            "api_base",
            "apiBase",
            "api_base_url",
            "apiBaseUrl",
        ],
    );
    let base_url_supplied = base_url_value.is_some();
    let base_url = safe_base_url(base_url_value.and_then(Value::as_str));
    let protocol_value = value_field(object, &["protocol", "wire_api", "wireApi"]);
    let protocol_supplied = protocol_value.is_some();
    let protocol = safe_protocol(protocol_value.and_then(Value::as_str));
    let priority = object
        .get("priority")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok());
    if metadata_was_rejected(
        object,
        base_url.as_deref(),
        protocol.as_deref(),
        plan_value,
        plan.as_deref(),
    ) {
        warnings.push(ImportWarning::new(
            ImportWarningCode::InvalidMetadataIgnored,
        ));
    }

    let email_value = email.as_deref();
    let identity_seed = if use_api_key {
        account_id
            .as_deref()
            .map(|value| format!("account:{}", value.to_ascii_lowercase()))
            .or_else(|| {
                email_value.map(|value| format!("email:{}", value.trim().to_ascii_lowercase()))
            })
    } else {
        token_identity_seed(
            account_id.as_deref(),
            chatgpt_user_id.as_deref(),
            email_value,
        )
    };
    let credential_fingerprint = if use_api_key {
        api_key.as_deref()
    } else if use_agent_identity {
        agent_private_key.as_deref()
    } else {
        refresh_token
            .as_deref()
            .or(access_token.as_deref())
            .or(id_token.as_deref())
    }
    .expect("selected credential presence checked");
    let identity_key = if let Some(identity_seed) = identity_seed.as_deref() {
        let identity_seed = if use_api_key {
            format!(
                "api:{}:{identity_seed}",
                base_url.as_deref().unwrap_or("default")
            )
        } else {
            format!("account:{identity_seed}")
        };
        sha256_hex(&identity_seed, None, None)
    } else {
        sha256_hex(
            if use_api_key {
                "api-key-without-identity"
            } else {
                "token-without-identity"
            },
            Some(credential_fingerprint),
            base_url.as_deref(),
        )
    };
    let item_seed = identity_seed
        .map(|identity_seed| {
            if use_api_key {
                format!(
                    "api:{}:{identity_seed}",
                    base_url.as_deref().unwrap_or("default")
                )
            } else {
                format!("account:{identity_seed}")
            }
        })
        .unwrap_or_else(|| {
            format!(
                "{}:{}:{}",
                source_file.unwrap_or("pasted"),
                ordinal,
                auth_mode.as_str()
            )
        });
    let item_id = format!("import_{}", &sha256_hex(&item_seed, None, None)[..16]);
    let identity = email_value
        .map(mask_email)
        .or_else(|| account_id.as_deref().map(mask_identifier))
        .unwrap_or_else(|| format!("imported-{}", ordinal + 1));
    let fallback_label = match auth_mode {
        ImportAuthMode::ApiKey => format!("API source {}", ordinal + 1),
        _ => format!("Account {}", ordinal + 1),
    };
    let label_value = string_field(
        object,
        &["name", "label", "api_provider_name", "apiProviderName"],
    )
    .or_else(|| meta.and_then(|data| string_field(data, &["name", "label"])));
    let mut label = safe_label(label_value).unwrap_or_else(|| identity.clone());
    if label == "unknown" || label.is_empty() {
        label = fallback_label;
    }
    label = redact_label_secrets(
        label,
        [
            api_key.as_deref(),
            access_token.as_deref(),
            refresh_token.as_deref(),
            id_token.as_deref(),
            agent_private_key.as_deref(),
            email_value,
        ],
        &identity,
    );
    let sensitive_values = [
        api_key.as_deref(),
        access_token.as_deref(),
        refresh_token.as_deref(),
        id_token.as_deref(),
        agent_private_key.as_deref(),
        email_value,
    ];
    redact_optional_metadata(&mut plan, &sensitive_values);
    redact_optional_metadata(&mut expires_at, &sensitive_values);
    redact_optional_metadata(&mut subscription_expires_at, &sensitive_values);
    let preview_source_file =
        source_file.map(|source_file| redact_file_name_with(source_file, &sensitive_values));

    let source_name = match format {
        ImportFormat::PortableAccountBundleV1 => "portable_account_bundle",
        ImportFormat::ZenithV1 => "zenith",
        _ if explicit_auth_mode.is_some() => "codex_auth_json",
        _ if use_api_key => "api_key_json",
        _ => "token_json",
    }
    .to_string();
    let secrets = ImportSecretMaterial {
        access_token: if use_tokens {
            access_token.map(RedactedValue::new)
        } else {
            None
        },
        refresh_token: if use_tokens {
            refresh_token.map(RedactedValue::new)
        } else {
            None
        },
        id_token: if use_tokens {
            id_token.map(RedactedValue::new)
        } else {
            None
        },
        api_key: if use_api_key {
            api_key.map(RedactedValue::new)
        } else {
            None
        },
        agent_private_key: if use_agent_identity {
            agent_private_key.map(RedactedValue::new)
        } else {
            None
        },
        agent_runtime_id: if use_agent_identity {
            agent_runtime_id.map(RedactedValue::new)
        } else {
            None
        },
        agent_task_id: if use_agent_identity {
            agent_task_id.map(RedactedValue::new)
        } else {
            None
        },
    };
    let preview = ImportPreviewRow {
        item_id: item_id.clone(),
        source_file: preview_source_file,
        label: label.clone(),
        identity,
        auth_mode,
        source_name,
        quota_status: ImportQuotaStatus::Skipped,
        status: ImportPreviewStatus::Ready,
        plan,
        expires_at,
        subscription_expires_at,
        error: None,
        default_selected: true,
        selectable: true,
        existing: false,
        warnings,
    };
    let item = ParsedImportItem {
        item_id,
        identity_key,
        label,
        account_id,
        chatgpt_user_id,
        organization_id,
        base_url,
        base_url_supplied,
        protocol,
        protocol_supplied,
        priority,
        email: email.map(RedactedValue::new),
        secrets,
    };
    Ok(ParsedItem { preview, item })
}

fn token_identity_seed(
    account_id: Option<&str>,
    user_id: Option<&str>,
    email: Option<&str>,
) -> Option<String> {
    let account = account_id.map(|value| value.trim().to_ascii_lowercase());
    let user = user_id.map(|value| value.trim().to_ascii_lowercase());
    let email = email.map(|value| value.trim().to_ascii_lowercase());
    match (account, email, user) {
        (Some(account), Some(email), _) => Some(format!("account:{account}:email:{email}")),
        (Some(account), None, Some(user)) => Some(format!("account:{account}:user:{user}")),
        (Some(account), None, None) => Some(format!("account:{account}")),
        (None, Some(email), _) => Some(format!("email:{email}")),
        (None, None, Some(user)) => Some(format!("user:{user}")),
        (None, None, None) => None,
    }
}

#[derive(Default)]
struct ImportedJwtMetadata {
    email: Option<String>,
    account_id: Option<String>,
    user_id: Option<String>,
    plan_type: Option<String>,
    expires_at: Option<String>,
    subscription_expires_at: Option<String>,
}

fn imported_jwt_metadata(
    id_token: Option<&str>,
    access_token: Option<&str>,
) -> ImportedJwtMetadata {
    let mut metadata = ImportedJwtMetadata::default();
    for token in [id_token, access_token].into_iter().flatten() {
        let Some(claims) =
            crate::accounts::decode_unverified_jwt_payload::<Value>(token).filter(Value::is_object)
        else {
            continue;
        };
        let auth = claims
            .get("https://api.openai.com/auth")
            .and_then(Value::as_object);
        let profile = claims
            .get("https://api.openai.com/profile")
            .and_then(Value::as_object);
        metadata.email = metadata
            .email
            .or_else(|| safe_metadata(claims.get("email").and_then(Value::as_str)))
            .or_else(|| {
                profile
                    .and_then(|profile| safe_metadata(profile.get("email").and_then(Value::as_str)))
            });
        metadata.account_id = metadata.account_id.or_else(|| {
            auth.and_then(|auth| {
                safe_identifier(string_field(auth, &["chatgpt_account_id", "account_id"]))
            })
        });
        metadata.user_id = metadata.user_id.or_else(|| {
            auth.and_then(|auth| {
                safe_identifier(string_field(auth, &["chatgpt_user_id", "user_id"]))
            })
        });
        metadata.plan_type = metadata.plan_type.or_else(|| {
            auth.and_then(|auth| {
                safe_metadata(auth.get("chatgpt_plan_type").and_then(Value::as_str))
            })
        });
        metadata.subscription_expires_at = metadata.subscription_expires_at.or_else(|| {
            auth.and_then(|auth| safe_expiry(auth.get("chatgpt_subscription_active_until")))
        });
        metadata.expires_at = metadata
            .expires_at
            .or_else(|| safe_expiry(claims.get("exp")));
    }
    metadata
}

fn is_oauth_mode(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "chatgpt" | "oauth" | "openai_oauth"
    )
}

fn is_agent_identity_mode(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "agentidentity" | "agent_identity"
    )
}

fn is_api_key_mode(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "apikey" | "api_key"
    )
}

fn is_token_mode(value: &str) -> bool {
    is_oauth_mode(value)
        || is_agent_identity_mode(value)
        || matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "token" | "imported_token" | "apikey" | "api_key"
        )
}

const ACCESS_TOKEN_FIELDS: &[&str] = &["access_token", "accessToken"];
const REFRESH_TOKEN_FIELDS: &[&str] = &["refresh_token", "refreshToken"];
const ID_TOKEN_FIELDS: &[&str] = &["id_token", "idToken"];
const API_KEY_FIELDS: &[&str] = &["OPENAI_API_KEY", "openai_api_key", "api_key", "apiKey"];
const AGENT_PRIVATE_KEY_FIELDS: &[&str] = &["agent_private_key", "agentPrivateKey"];
const AGENT_RUNTIME_ID_FIELDS: &[&str] = &["agent_runtime_id", "agentRuntimeId"];
const AGENT_TASK_ID_FIELDS: &[&str] = &["task_id", "taskId"];
const EMAIL_FIELDS: &[&str] = &["email", "identity_email"];
const ACCOUNT_ID_FIELDS: &[&str] = &[
    "chatgpt_account_id",
    "chatgptAccountId",
    "account_id",
    "accountId",
];
const USER_ID_FIELDS: &[&str] = &["chatgpt_user_id", "chatgptUserId", "user_id", "userId"];
const ORGANIZATION_ID_FIELDS: &[&str] = &[
    "organization_id",
    "organizationId",
    "org_id",
    "orgId",
    "poid",
    "POID",
];
const PLAN_FIELDS: &[&str] = &[
    "chatgpt_plan_type",
    "chatgptPlanType",
    "plan_type",
    "planType",
    "plan",
];
const EXPIRES_AT_FIELDS: &[&str] = &["expires_at", "expiresAt", "expired"];
const SUBSCRIPTION_EXPIRES_AT_FIELDS: &[&str] = &[
    "subscription_expires_at",
    "subscriptionExpiresAt",
    "subscription_active_until",
    "subscriptionActiveUntil",
    "chatgpt_subscription_active_until",
    "chatgptSubscriptionActiveUntil",
];

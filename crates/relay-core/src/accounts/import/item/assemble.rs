use super::super::sanitization::*;
use super::super::*;
use super::identity::{stable_import_identity, ImportIdentity, ImportIdentityInput};
use super::profile::ImportProfile;

pub(super) struct AssembleImport<'a> {
    pub(super) ordinal: usize,
    pub(super) format: ImportFormat,
    pub(super) source_file: Option<&'a str>,
    pub(super) import_object: &'a Map<String, Value>,
    pub(super) meta: Option<&'a Map<String, Value>>,
    pub(super) tags_value: Option<&'a Value>,
    pub(super) use_api_key: bool,
    pub(super) use_tokens: bool,
    pub(super) use_agent_identity: bool,
    pub(super) auth_mode: ImportAuthMode,
    pub(super) explicit_auth_declared: bool,
    pub(super) api_key: Option<String>,
    pub(super) access_token: Option<String>,
    pub(super) refresh_token: Option<String>,
    pub(super) id_token: Option<String>,
    pub(super) agent_private_key: Option<String>,
    pub(super) agent_runtime_id: Option<String>,
    pub(super) agent_task_id: Option<String>,
    pub(super) profile: ImportProfile,
    pub(super) warnings: Vec<ImportWarning>,
}

pub(super) fn assemble_parsed_item(
    import_input: AssembleImport<'_>,
) -> Result<ParsedItem, ImportIssue> {
    let AssembleImport {
        ordinal,
        format,
        source_file,
        import_object,
        meta,
        tags_value,
        use_api_key,
        use_tokens,
        use_agent_identity,
        auth_mode,
        explicit_auth_declared,
        api_key,
        access_token,
        refresh_token,
        id_token,
        agent_private_key,
        agent_runtime_id,
        agent_task_id,
        profile:
            ImportProfile {
                email,
                phone,
                password,
                totp_secret,
                account_id,
                chatgpt_user_id,
                organization_id,
                mut plan,
                mut expires_at,
                mut subscription_expires_at,
                base_url,
                base_url_supplied,
                protocol,
                protocol_supplied,
                priority,
                account_is_fedramp,
                metadata_rejected,
            },
        mut warnings,
    } = import_input;
    let email_value = email.as_deref();
    let ImportIdentity {
        identity_key,
        item_id,
    } = stable_import_identity(ImportIdentityInput {
        ordinal,
        source_file,
        use_api_key,
        use_agent_identity,
        auth_mode: &auth_mode,
        api_key: api_key.as_deref(),
        access_token: access_token.as_deref(),
        refresh_token: refresh_token.as_deref(),
        id_token: id_token.as_deref(),
        agent_private_key: agent_private_key.as_deref(),
        account_id: account_id.as_deref(),
        chatgpt_user_id: chatgpt_user_id.as_deref(),
        email: email_value,
        base_url: base_url.as_deref(),
    })?;
    let identity = email_value
        .map(mask_email)
        .or_else(|| account_id.as_deref().map(mask_identifier))
        .unwrap_or_else(|| format!("imported-{}", ordinal + 1));
    let fallback_label = match auth_mode {
        ImportAuthMode::ApiKey => format!("API source {}", ordinal + 1),
        _ => format!("Account {}", ordinal + 1),
    };
    let label_value = string_field(
        import_object,
        &[
            "name",
            "label",
            "account_name",
            "accountName",
            "api_provider_name",
            "apiProviderName",
        ],
    )
    .or_else(|| meta.and_then(|metadata_object| string_field(metadata_object, &["name", "label"])));
    let mut label = safe_label(label_value).unwrap_or_else(|| identity.clone());
    if label == "unknown" || label.is_empty() {
        label = fallback_label;
    }
    let sensitive_values = [
        api_key.as_deref(),
        access_token.as_deref(),
        refresh_token.as_deref(),
        id_token.as_deref(),
        agent_private_key.as_deref(),
        email_value,
        phone.as_deref(),
        password.as_deref(),
        totp_secret.as_deref(),
    ];
    label = redact_label_secrets(label, sensitive_values, &identity);
    let (tags, tags_rejected) = safe_import_tags(tags_value, &sensitive_values);
    if metadata_rejected || tags_rejected {
        warnings.push(ImportWarning::new(
            ImportWarningCode::InvalidMetadataIgnored,
        ));
    }
    redact_optional_metadata(&mut plan, &sensitive_values);
    redact_optional_metadata(&mut expires_at, &sensitive_values);
    redact_optional_metadata(&mut subscription_expires_at, &sensitive_values);
    let preview_source_file =
        source_file.map(|source_file| redact_file_name_with(source_file, &sensitive_values));

    let source_name = match format {
        ImportFormat::PortableAccountBundleV1 => "portable_account_bundle",
        ImportFormat::ZenithV1 => "zenith",
        _ if explicit_auth_declared => "codex_auth_json",
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
    let parsed_import_item = ParsedImportItem {
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
        account_is_fedramp,
        tags,
        email: email.map(RedactedValue::new),
        phone: phone.map(RedactedValue::new),
        password: password.map(RedactedValue::new),
        totp_secret: totp_secret.map(RedactedValue::new),
        secrets,
    };
    Ok(ParsedItem {
        preview,
        parsed_item: parsed_import_item,
    })
}

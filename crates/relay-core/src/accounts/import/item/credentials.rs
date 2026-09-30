use super::super::*;
use super::vocabulary::*;

pub(super) struct ImportCredentialChoice {
    pub(super) use_api_key: bool,
    pub(super) use_tokens: bool,
    pub(super) use_agent_identity: bool,
    pub(super) auth_mode: ImportAuthMode,
    pub(super) explicit_auth_declared: bool,
    pub(super) warnings: Vec<ImportWarning>,
}

pub(super) fn choose_import_credentials(
    has_api_key: bool,
    has_tokens: bool,
    has_agent_identity: bool,
    access_token_present: bool,
    refresh_token_present: bool,
    explicit_auth_mode: Option<&str>,
) -> Result<ImportCredentialChoice, ImportIssue> {
    let explicit_oauth = explicit_auth_mode.is_some_and(is_oauth_mode);
    let explicit_agent_identity = explicit_auth_mode.is_some_and(is_agent_identity_mode);
    let explicit_api_key = explicit_auth_mode.is_some_and(is_api_key_mode);
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
    if use_tokens && !access_token_present && !refresh_token_present {
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
    if use_tokens && access_token_present && !refresh_token_present {
        warnings.push(ImportWarning::new(ImportWarningCode::AccessTokenOnly));
    }
    if use_tokens && !access_token_present && refresh_token_present {
        warnings.push(ImportWarning::new(
            ImportWarningCode::RefreshExchangeRequired,
        ));
    }
    Ok(ImportCredentialChoice {
        use_api_key,
        use_tokens,
        use_agent_identity,
        auth_mode,
        explicit_auth_declared: explicit_auth_mode.is_some(),
        warnings,
    })
}

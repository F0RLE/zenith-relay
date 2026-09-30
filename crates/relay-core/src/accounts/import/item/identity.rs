use super::super::sanitization::sha256_hex;
use super::super::*;
use super::jwt::{chatgpt_token_identity_key, token_identity_seed};

pub(super) struct ImportIdentityInput<'a> {
    pub(super) ordinal: usize,
    pub(super) source_file: Option<&'a str>,
    pub(super) use_api_key: bool,
    pub(super) use_agent_identity: bool,
    pub(super) auth_mode: &'a ImportAuthMode,
    pub(super) api_key: Option<&'a str>,
    pub(super) access_token: Option<&'a str>,
    pub(super) refresh_token: Option<&'a str>,
    pub(super) id_token: Option<&'a str>,
    pub(super) agent_private_key: Option<&'a str>,
    pub(super) account_id: Option<&'a str>,
    pub(super) chatgpt_user_id: Option<&'a str>,
    pub(super) email: Option<&'a str>,
    pub(super) base_url: Option<&'a str>,
}

pub(super) struct ImportIdentity {
    pub(super) identity_key: String,
    pub(super) item_id: String,
}

pub(super) fn stable_import_identity(
    input: ImportIdentityInput<'_>,
) -> Result<ImportIdentity, ImportIssue> {
    let ImportIdentityInput {
        ordinal,
        source_file,
        use_api_key,
        use_agent_identity,
        auth_mode,
        api_key,
        access_token,
        refresh_token,
        id_token,
        agent_private_key,
        account_id,
        chatgpt_user_id,
        email,
        base_url,
    } = input;
    let identity_seed = if use_api_key {
        account_id
            .map(|value| format!("account:{}", value.to_ascii_lowercase()))
            .or_else(|| email.map(|value| format!("email:{}", value.trim().to_ascii_lowercase())))
    } else {
        token_identity_seed(account_id, chatgpt_user_id, email)
    };
    let credential_fingerprint = if use_api_key {
        api_key
    } else if use_agent_identity {
        agent_private_key
    } else {
        refresh_token.or(access_token).or(id_token)
    }
    .ok_or_else(|| {
        ImportIssue::new(
            ImportIssueCode::MissingCredentials,
            "import item has no supported credential",
        )
    })?;
    let identity_key = if !use_api_key {
        chatgpt_token_identity_key(account_id, chatgpt_user_id, email).unwrap_or_else(|| {
            sha256_hex(
                "token-without-identity",
                Some(credential_fingerprint),
                base_url,
            )
        })
    } else if let Some(identity_seed) = identity_seed.as_deref() {
        sha256_hex(
            &format!("api:{}:{identity_seed}", base_url.unwrap_or("default")),
            None,
            None,
        )
    } else {
        sha256_hex(
            "api-key-without-identity",
            Some(credential_fingerprint),
            base_url,
        )
    };
    let item_seed = identity_seed
        .map(|identity_seed| {
            if use_api_key {
                format!("api:{}:{identity_seed}", base_url.unwrap_or("default"))
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
    Ok(ImportIdentity {
        identity_key,
        item_id,
    })
}

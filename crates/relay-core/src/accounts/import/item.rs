use super::*;

mod assemble;
mod credentials;
mod identity;
mod jwt;
mod present;
mod profile;
mod vocabulary;

use assemble::{assemble_parsed_item, AssembleImport};
use credentials::{choose_import_credentials, ImportCredentialChoice};
pub use jwt::chatgpt_token_identity_key;
use jwt::imported_jwt_metadata;
use present::{present_import_item, PresentedImport};
use profile::{read_import_profile, ImportFieldSources};

pub(super) fn parse_item(
    import_item_json: &Value,
    ordinal: usize,
    format: ImportFormat,
    source_file: Option<&str>,
) -> Result<ParsedItem, ImportIssue> {
    let import_object = import_item_json.as_object().ok_or_else(|| {
        ImportIssue::new(
            ImportIssueCode::UnsupportedValue,
            "import item must be a JSON object",
        )
    })?;
    if import_object
        .get(IMPORT_ERROR_MARKER)
        .is_some_and(Value::is_boolean)
    {
        return Err(ImportIssue::new(
            ImportIssueCode::MalformedJson,
            "import file or line is malformed",
        ));
    }
    let PresentedImport {
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
    } = present_import_item(import_object, format)?;
    let has_api_key = api_key.is_some();
    let has_tokens = access_token.is_some() || refresh_token.is_some() || id_token.is_some();
    let has_agent_identity =
        agent_private_key.is_some() || agent_runtime_id.is_some() || agent_task_id.is_some();
    if has_agent_identity && (agent_private_key.is_none() || agent_runtime_id.is_none()) {
        return Err(ImportIssue::new(
            ImportIssueCode::InvalidCredentials,
            "Agent Identity import requires a private key and runtime id",
        ));
    }
    let ImportCredentialChoice {
        use_api_key,
        use_tokens,
        use_agent_identity,
        auth_mode,
        explicit_auth_declared,
        mut warnings,
    } = choose_import_credentials(
        has_api_key,
        has_tokens,
        has_agent_identity,
        access_token.is_some(),
        refresh_token.is_some(),
        explicit_auth_mode,
    )?;
    let jwt = imported_jwt_metadata(id_token.as_deref(), access_token.as_deref());
    if import_object.contains_key("concurrency") {
        warnings.push(ImportWarning::new(ImportWarningCode::ConcurrencyIgnored));
    }

    let profile = read_import_profile(
        ImportFieldSources {
            import_object,
            credentials,
            agent_identity: agent_identity_object,
            account,
            provider: provider_object,
            identity,
            meta,
            subscription,
            user,
            session_profile,
            header_account_id,
        },
        jwt,
    );

    assemble_parsed_item(AssembleImport {
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
        profile,
        warnings,
    })
}

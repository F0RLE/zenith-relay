use super::{CommandResult, ImportItemError, ItemResult, MAX_ACCOUNT_LABEL_BYTES, MAX_MODELS};
use crate::local_pool::accounts::quota_refresh::QUOTA_REFRESH_BATCH_SIZE;
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::models::LocalAccountRecord;
use std::collections::HashSet;
use zenith_relay_core::accounts::{
    AccountAuthMode, AccountHealthState, ImportAuthMode, ParsedImportItem,
};
use zenith_relay_core::is_valid_model_id;

pub(in crate::local_pool::accounts) fn ensure_account_import_item(
    item: &ParsedImportItem,
) -> ItemResult<()> {
    if item.secrets().api_key().is_some() {
        Err(ImportItemError::new(
            "use_source_import",
            "API keys must be imported as compatible API sources",
        ))
    } else {
        Ok(())
    }
}

pub(in crate::local_pool::accounts) fn account_auth_mode(
    mode: ImportAuthMode,
) -> ItemResult<AccountAuthMode> {
    match mode {
        ImportAuthMode::OAuth => Ok(AccountAuthMode::OAuth),
        ImportAuthMode::AgentIdentity => Ok(AccountAuthMode::ImportedToken),
        ImportAuthMode::ImportedToken => Ok(AccountAuthMode::ImportedToken),
        ImportAuthMode::ApiKey => Err(ImportItemError::new(
            "use_source_import",
            "API keys must be imported as compatible API sources",
        )),
        ImportAuthMode::Unknown => Err(ImportItemError::new(
            "unknown_auth_mode",
            "imported account authentication mode is unknown",
        )),
    }
}

pub(in crate::local_pool::accounts) fn merge_existing_account(
    account: &mut LocalAccountRecord,
    existing: Option<&LocalAccountRecord>,
) {
    let Some(existing) = existing else {
        return;
    };
    account.account.label = existing.account.label.clone();
    account.account.tags = existing.account.tags.clone();
    account.account.enabled = existing.account.enabled;
    account.account.in_pool = existing.account.in_pool;
    account.account.draining = existing.account.draining;
    account.account.created_at_ms = existing.account.created_at_ms;
    account.account.last_used_at_ms = existing.account.last_used_at_ms;
    account.account.health = existing.account.health;
    account.account.quota = existing.account.quota.clone();
    account.account.subscription = existing.account.subscription.clone();
    account.account.last_error_code = existing.account.last_error_code.clone();
    account.remote_location = existing.remote_location.clone();
    account.allowed_models = existing.allowed_models.clone();
    account.excluded_models = existing.excluded_models.clone();
    account.priority = existing.priority;
    account.weight = existing.weight;
}

pub(in crate::local_pool::accounts) fn preserve_newer_account_state(
    account: &mut LocalAccountRecord,
    before_refresh: &LocalAccountRecord,
    current: &LocalAccountRecord,
) {
    if current.account.auth_state != before_refresh.account.auth_state {
        account.account.auth_state = current.account.auth_state;
    }
    if current.account.health != before_refresh.account.health {
        account.account.health = current.account.health;
    }
    if current.account.last_error_code != before_refresh.account.last_error_code {
        account.account.last_error_code = current.account.last_error_code.clone();
    }
}

pub(in crate::local_pool::accounts) fn account_model_state_is_valid(
    account: &LocalAccountRecord,
) -> bool {
    !account.effective_models().is_empty()
        || (account.account.last_error_code.is_some()
            && account.account.health != AccountHealthState::Healthy)
}

pub(in crate::local_pool::accounts) fn validate_label(label: &str) -> LocalResult<()> {
    let label = label.trim();
    if label.is_empty()
        || label.len() > MAX_ACCOUNT_LABEL_BYTES
        || label.chars().any(char::is_control)
    {
        Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "account label is invalid",
        ))
    } else {
        Ok(())
    }
}

pub(in crate::local_pool::accounts) fn normalize_models(
    models: Vec<String>,
) -> LocalResult<Vec<String>> {
    if models.len() > MAX_MODELS {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "model list exceeds the supported limit",
        ));
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for model in models {
        let model = model.trim();
        if model.is_empty() {
            continue;
        }
        if !is_valid_model_id(model) {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "model name is invalid",
            ));
        }
        if seen.insert(model.to_string()) {
            normalized.push(model.to_string());
        }
    }
    Ok(normalized)
}

pub(in crate::local_pool::accounts) fn normalize_selected_item_ids(
    item_ids: Vec<String>,
) -> CommandResult<Vec<String>> {
    if item_ids.len() > super::MAX_IMPORT_ITEMS {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "selected import item count exceeds the supported limit",
        )
        .into());
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for item_id in item_ids {
        let item_id = item_id.trim();
        let Some(suffix) = item_id.strip_prefix("import_") else {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "selected import item id is invalid",
            )
            .into());
        };
        if suffix.len() != 16 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "selected import item id is invalid",
            )
            .into());
        }
        if seen.insert(item_id.to_string()) {
            normalized.push(item_id.to_string());
        }
    }
    Ok(normalized)
}

pub(in crate::local_pool::accounts) fn should_probe_import_quota(
    requested: bool,
    row_count: usize,
) -> bool {
    requested && row_count <= QUOTA_REFRESH_BATCH_SIZE
}

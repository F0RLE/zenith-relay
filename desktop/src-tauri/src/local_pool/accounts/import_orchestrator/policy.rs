use super::{CommandResult, MAX_MODELS};
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::models::LocalAccountRecord;
use zenith_relay_core::providers::chatgpt::connector::import as shared;
use zenith_relay_core::{ModelIdListError, normalize_bounded_model_ids};

pub(in crate::local_pool::accounts) fn preserve_newer_account_state(
    account: &mut LocalAccountRecord,
    before_refresh: &LocalAccountRecord,
    latest_account: &LocalAccountRecord,
) {
    zenith_relay_core::providers::chatgpt::connector::observations::preserve_newer_account_state(
        &mut account.account,
        &before_refresh.account,
        &latest_account.account,
    );
}

pub(in crate::local_pool::accounts) fn account_model_state_is_valid(
    account: &LocalAccountRecord,
) -> bool {
    shared::account_model_state_is_valid(account)
}

pub(in crate::local_pool::accounts) fn validate_label(label: &str) -> LocalResult<()> {
    if !shared::valid_label(label) {
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
    normalize_bounded_model_ids(models, MAX_MODELS).map_err(|error| {
        LocalPoolError::new(
            ErrorCode::InvalidState,
            match error {
                ModelIdListError::TooLarge => "model list exceeds the supported limit",
                ModelIdListError::InvalidId => "model name is invalid",
            },
        )
    })
}

pub(in crate::local_pool::accounts) fn normalize_selected_item_ids(
    item_ids: Vec<String>,
) -> CommandResult<Vec<String>> {
    zenith_relay_core::accounts::normalize_selected_item_ids(item_ids)
        .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message).into())
}

pub(in crate::local_pool::accounts) fn should_probe_import_quota(
    requested: bool,
    row_count: usize,
) -> bool {
    shared::should_probe_quota(requested, row_count)
}

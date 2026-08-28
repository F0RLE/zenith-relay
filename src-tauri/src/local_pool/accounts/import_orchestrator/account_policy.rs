use super::{
    account_model_state_is_valid, credential_local_error, normalize_models, validate_label,
};
use crate::local_pool::accounts::credentials::CredentialStore;
use crate::local_pool::accounts::mutations::UpdateAccountInput;
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::models::LocalAccountRecord;
use zenith_relay_core::accounts::MAX_PURCHASE_COST_MICRO_USD;

pub(in crate::local_pool::accounts) fn apply_account_patch(
    account: &mut LocalAccountRecord,
    input: UpdateAccountInput,
) -> LocalResult<()> {
    if let Some(label) = input.label {
        validate_label(&label)?;
        account.account.label = label;
    }
    if let Some(priority) = input.priority {
        account.priority = priority;
    }
    if let Some(weight) = input.weight {
        if weight == 0 {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "account weight must be positive",
            ));
        }
        account.weight = weight;
    }
    if let Some(models) = input.allowed_models {
        account.allowed_models = normalize_models(models)?;
    }
    if let Some(models) = input.excluded_models {
        account.excluded_models = normalize_models(models)?;
    }
    if let Some(in_pool) = input.in_pool {
        account.account.in_pool = in_pool;
    }
    if let Some(draining) = input.draining {
        account.account.draining = draining;
    }
    if let Some(purchase_cost) = input.purchase_cost_micro_usd {
        if purchase_cost > MAX_PURCHASE_COST_MICRO_USD {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "account purchase cost is too large",
            ));
        }
        account.purchase_cost_micro_usd = (purchase_cost > 0).then_some(purchase_cost);
    }
    account.normalize();
    Ok(())
}

pub(in crate::local_pool::accounts) fn validate_account_record(
    account: &LocalAccountRecord,
) -> LocalResult<()> {
    validate_label(&account.account.label)?;
    if let Some(location) = &account.remote_location {
        if location.server_id.is_empty() || location.remote_account_id.is_empty() {
            return Err(LocalPoolError::new(
                ErrorCode::InvalidState,
                "remote account location is invalid",
            ));
        }
        if account.account.enabled || account.account.in_pool {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "an account managed by a remote server cannot run locally",
            ));
        }
    }
    if !account_model_state_is_valid(account) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "a healthy account must expose at least one model",
        ));
    }
    normalize_models(account.models.clone())?;
    normalize_models(account.allowed_models.clone())?;
    normalize_models(account.excluded_models.clone())?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend)
        .require(&account.account.id)
        .map_err(credential_local_error)?;
    if credentials.provider_account_id().is_none() {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "account credentials do not contain a provider account id",
        ));
    }
    if credentials.has_oauth() {
        credentials.to_token_set().map_err(credential_local_error)?;
    }
    Ok(())
}

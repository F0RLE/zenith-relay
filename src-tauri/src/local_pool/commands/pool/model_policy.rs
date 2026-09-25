use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result as LocalResult},
    models::{LocalAccountRecord, ProviderSourceRecord},
    state::DesktopState,
};
use std::collections::BTreeSet;
use zenith_relay_core::protocol::{canonical_pool_model_id, ModelPolicyError};

/// Resolve a user-facing model id to the canonical casing used by the pool.
/// Sources are preferred because they carry the protocol-specific model map;
/// native accounts remain a fallback for personal ChatGPT capacity.
pub(crate) fn canonical_pool_model(state: &DesktopState, model_id: &str) -> LocalResult<String> {
    let store = state.store()?;
    canonical_pool_model_id(
        configured_pool_model_ids(store.sources(), store.accounts()),
        model_id,
    )
    .map(str::to_owned)
    .map_err(model_policy_error)
}

/// Model edits use complete configured membership, including unavailable models
/// and binding-only IDs. Runtime admission remains independent.
pub(super) fn configured_pool_model_ids<'a>(
    sources: &'a [ProviderSourceRecord],
    accounts: &'a [LocalAccountRecord],
) -> impl Iterator<Item = &'a String> {
    let source_models = sources
        .iter()
        .filter(|source| source.in_pool)
        .flat_map(|source| {
            source.models.iter().chain(
                source
                    .protocol_bindings
                    .iter()
                    .flat_map(|binding| &binding.model_ids),
            )
        });
    let account_models = accounts
        .iter()
        .filter(|account| account.account.in_pool)
        .flat_map(LocalAccountRecord::effective_models);
    source_models.chain(account_models)
}

pub(super) fn model_policy_error(error: ModelPolicyError) -> LocalPoolError {
    let code = match error {
        ModelPolicyError::NotFound => ErrorCode::NotFound,
        ModelPolicyError::InvalidId | ModelPolicyError::DuplicateOrderEntry => {
            ErrorCode::InvalidState
        }
    };
    LocalPoolError::new(code, error.to_string())
}

/// Return configured pool members without consulting live scheduler health.
/// Availability is intentionally a runtime concern and is applied later.
pub(crate) fn local_pool_member_ids(
    sources: &[ProviderSourceRecord],
    accounts: &[LocalAccountRecord],
) -> LocalResult<(BTreeSet<String>, BTreeSet<String>)> {
    let mut source_ids = BTreeSet::new();
    for source in sources.iter().filter(|source| source.in_pool) {
        // A damaged legacy binding is an unavailable candidate, not a reason
        // to reject the complete local key scope. Runtime admission records a
        // stable source error and keeps the remaining pool routes usable.
        if source.supports_any_wire_api().unwrap_or(false) {
            source_ids.insert(source.id.clone());
        }
    }
    let account_ids = accounts
        .iter()
        .filter(|account| account.account.in_pool && account.remote_location.is_none())
        .map(|account| account.account.id.clone())
        .collect();
    Ok((source_ids, account_ids))
}

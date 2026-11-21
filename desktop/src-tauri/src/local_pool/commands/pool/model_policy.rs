use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result as LocalResult},
    models::{LocalAccountRecord, ProviderSourceRecord},
    state::DesktopState,
};
use zenith_relay_core::protocol::{
    ModelPolicyError, canonical_pool_model_id, configured_source_model_ids,
};

/// Resolve a user-facing model id to the canonical casing used by the pool.
/// Sources are preferred because they carry the protocol-specific model map;
/// native accounts remain a fallback for personal ChatGPT capacity.
pub(crate) fn canonical_pool_model(state: &DesktopState, model_id: &str) -> LocalResult<String> {
    let store = state.store()?;
    canonical_pool_model_id(
        configured_pool_model_ids(
            store.sources(),
            store.accounts(),
            store.gateway().codex_text_models_enabled,
        ),
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
    codex_text_models_enabled: bool,
) -> impl Iterator<Item = &'a String> {
    let source_models = sources
        .iter()
        .filter(|source| source.in_pool)
        .flat_map(|source| configured_source_model_ids(&source.models, &source.protocol_bindings));
    let account_models = accounts
        .iter()
        .filter(|account| account.account.in_pool)
        .flat_map(move |account| account.pool_models(codex_text_models_enabled));
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

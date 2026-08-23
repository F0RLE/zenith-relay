use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result as LocalResult},
    models::{LocalAccountRecord, ProviderSourceRecord},
    state::DesktopState,
};
use std::collections::BTreeSet;
use zenith_relay_core::{is_valid_model_id, WireApi};

/// Resolve a user-facing model id to the canonical casing used by the pool.
/// Sources are preferred because they carry the protocol-specific model map;
/// native accounts remain a fallback for personal ChatGPT capacity.
pub(crate) fn canonical_pool_model(state: &DesktopState, model_id: &str) -> LocalResult<String> {
    let requested = model_id.trim();
    if !is_valid_model_id(requested) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "model id is invalid",
        ));
    }
    let store = state.store()?;
    for source in store.sources().iter().filter(|source| source.in_pool) {
        let models = source
            .models_for_wire_api(WireApi::Responses)
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?;
        if let Some(model) = models
            .into_iter()
            .find(|model| model.eq_ignore_ascii_case(requested))
        {
            return Ok(model);
        }
    }
    store
        .accounts()
        .iter()
        .filter(|account| account.account.in_pool)
        .flat_map(|account| account.models.iter())
        .find(|model| model.eq_ignore_ascii_case(requested))
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "pool model not found"))
}

/// Return configured pool members without consulting live scheduler health.
/// Availability is intentionally a runtime concern and is applied later.
pub(crate) fn local_pool_member_ids(
    sources: &[ProviderSourceRecord],
    accounts: &[LocalAccountRecord],
) -> LocalResult<(BTreeSet<String>, BTreeSet<String>)> {
    let mut source_ids = BTreeSet::new();
    for source in sources.iter().filter(|source| source.in_pool) {
        if source
            .supports_wire_api(WireApi::Responses)
            .map_err(|message| LocalPoolError::new(ErrorCode::InvalidState, message))?
        {
            source_ids.insert(source.id.clone());
        }
    }
    let account_ids = accounts
        .iter()
        .filter(|account| account.account.in_pool)
        .map(|account| account.account.id.clone())
        .collect();
    Ok((source_ids, account_ids))
}

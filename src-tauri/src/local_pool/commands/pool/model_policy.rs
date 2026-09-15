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
        let mut models = Vec::new();
        for wire_api in WireApi::ALL {
            let Ok(source_models) = source.models_for_wire_api(wire_api) else {
                // A malformed source is shown as unavailable and must not
                // prevent model selection from the remaining pool members.
                continue;
            };
            models.extend(source_models);
        }
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
        .flat_map(|account| account.effective_models().iter())
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
        // A damaged legacy binding is an unavailable candidate, not a reason
        // to reject the complete local key scope. Runtime admission records a
        // stable source error and keeps the remaining pool routes usable.
        if source.supports_any_wire_api().unwrap_or(false) {
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

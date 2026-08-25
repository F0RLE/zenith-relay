use super::{WAKE_OUTPUT_TOKEN_CAP, WAKE_VERIFICATION_DELAY_MS};
use crate::local_pool::models::LocalAccountRecord;
use zenith_relay_core::{
    automations::{model_lightness_rank, WakeAdapterPolicy, WakeModel},
    quota::QuotaAdapterCapabilities,
};

pub(super) fn codex_wake_policy(
    account: &LocalAccountRecord,
    capabilities: &QuotaAdapterCapabilities,
) -> WakeAdapterPolicy {
    let models = account
        .effective_models()
        .iter()
        .filter(|model| model_allowed(account, model))
        .enumerate()
        .map(|(index, model)| WakeModel {
            id: model.clone(),
            lightness_rank: model_lightness_rank(model, index),
            wake_capable: true,
        })
        .collect();
    WakeAdapterPolicy {
        windows_requiring_activity: capabilities.wake_windows.clone(),
        models,
        verification_delay_ms: WAKE_VERIFICATION_DELAY_MS,
        output_token_cap: WAKE_OUTPUT_TOKEN_CAP,
    }
}

fn model_allowed(account: &LocalAccountRecord, model: &str) -> bool {
    (account.allowed_models.is_empty()
        || account
            .allowed_models
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(model)))
        && !account
            .excluded_models
            .iter()
            .any(|excluded| excluded.eq_ignore_ascii_case(model))
}

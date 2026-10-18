use super::{AccountCredential, AccountImportPreview, ManagementError, ServerAccountRecord};
use std::collections::BTreeMap;
use zenith_relay_core::accounts::{AccountAuthState, AccountHealthState};
use zenith_relay_core::error_codes;
use zenith_relay_core::quota::{Subscription, SubscriptionInput};

pub(super) struct ConfirmedAccountRecordInput<'a> {
    pub(super) preview: AccountImportPreview,
    pub(super) credential: &'a AccountCredential,
    pub(super) existing: Option<&'a ServerAccountRecord>,
    pub(super) secret_ref: String,
    pub(super) created_at_ms: u64,
    pub(super) observed_at_ms: u64,
    pub(super) add_to_pool: bool,
}

/// Project the validated import without performing vault, store or runtime I/O.
/// Existing operator settings win over the preview's defaults.
pub(super) fn confirmed_account_record(
    input: ConfirmedAccountRecordInput<'_>,
) -> Result<ServerAccountRecord, ManagementError> {
    let ConfirmedAccountRecordInput {
        preview,
        credential,
        existing,
        secret_ref,
        created_at_ms,
        observed_at_ms,
        add_to_pool,
    } = input;
    let auth_state = if credential.is_agent_identity() || credential.refresh_token.is_some() {
        AccountAuthState::Active
    } else {
        AccountAuthState::DegradedAccessOnly
    };
    let mut subscription =
        if preview.plan_type.is_some() || preview.subscription_active_until_ms.is_some() {
            Subscription::normalize(SubscriptionInput {
                plan_type: preview.plan_type.clone(),
                active_until_ms: preview.subscription_active_until_ms,
                forbidden: false,
                observed_at_ms,
            })
        } else {
            existing
                .map(|account| account.subscription.clone())
                .unwrap_or_default()
        };
    if subscription.active_until_ms.is_none() {
        subscription.updated_at_ms = None;
    }
    credential
        .validate_plan(subscription.plan_type.as_deref())
        .map_err(|message| {
            ManagementError::validation(error_codes::BASIS_POINTS_FREE_ACCOUNT_UNSUPPORTED, message)
        })?;
    Ok(ServerAccountRecord {
        id: preview.account_id,
        label: existing
            .map(|account| account.label.clone())
            .unwrap_or(preview.label),
        identity_hint: preview.identity_hint,
        enabled: existing.is_none_or(|account| account.enabled),
        in_pool: add_to_pool || existing.is_some_and(|account| account.in_pool),
        draining: existing.is_some_and(|account| account.draining),
        source_id: "openai_codex".to_string(),
        secret_ref,
        provider_family: existing
            .and_then(|account| account.provider_family.clone())
            .or_else(|| Some("openai".to_string())),
        auth_state,
        health: AccountHealthState::Healthy,
        models: existing
            .map(|account| account.models.clone())
            .unwrap_or(preview.models),
        discovered_models: existing.and_then(|account| account.discovered_models.clone()),
        allowed_models: existing
            .map(|account| account.allowed_models.clone())
            .unwrap_or(preview.allowed_models),
        excluded_models: existing
            .map(|account| account.excluded_models.clone())
            .unwrap_or(preview.excluded_models),
        priority: existing.map_or(preview.priority, |account| account.priority),
        weight: existing.map_or(preview.weight, |account| account.weight),
        subscription,
        quota: existing
            .map(|account| account.quota.clone())
            .unwrap_or_default(),
        purchase_cost_micro_usd: existing.and_then(|account| account.purchase_cost_micro_usd),
        cooldowns: BTreeMap::new(),
        consecutive_failures: 0,
        created_at_ms: existing
            .map(|account| account.created_at_ms)
            .filter(|created_at_ms| *created_at_ms > 0)
            .unwrap_or(created_at_ms),
        last_used_at_ms: existing.and_then(|account| account.last_used_at_ms),
        last_error_code: None,
        proxy_id: existing.and_then(|account| account.proxy_id.clone()),
        bypass_common_proxy: existing.is_some_and(|account| account.bypass_common_proxy),
    })
}

#[cfg(test)]
mod tests;

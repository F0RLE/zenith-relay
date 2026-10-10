use super::windows::normalize_subscription_plan;
use super::{
    QuotaNormalizationError, QuotaSnapshot, QuotaWindow, QuotaWindowInput, QuotaWindowKind,
    Subscription, SubscriptionInput, SupplementalQuotaWindow,
};
use crate::DefaultServiceTier;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

mod failure;

pub use failure::{classify_quota_http_failure, QuotaRefreshFailure};
use std::collections::{BTreeSet, HashSet};

const MAX_SUPPLEMENTAL_WINDOWS: usize = 32;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupplementalQuotaWindowInput {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<DefaultServiceTier>,
    pub window: QuotaWindowInput,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaAdapterCapabilities {
    pub supports_quota: bool,
    pub supports_subscription: bool,
    pub supported_windows: BTreeSet<QuotaWindowKind>,
    pub wake_windows: BTreeSet<QuotaWindowKind>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaAdapterContext {
    pub account_id: String,
    pub source_id: String,
    /// Provider-owned account identity used by the adapter. This is never a
    /// UI label or a scheduler id.
    pub provider_account_id: String,
    pub stable_identity: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaRefreshData {
    pub primary: Option<QuotaWindowInput>,
    pub secondary: Option<QuotaWindowInput>,
    #[serde(default)]
    pub supplemental: Vec<SupplementalQuotaWindowInput>,
    #[serde(default)]
    /// Normalized value persisted into `QuotaSnapshot`.
    pub limit_reached: bool,
    pub subscription: Option<SubscriptionInput>,
    pub reset_credits_available: Option<u32>,
    /// Provider-reported credits, expressed in millionths of one credit.
    /// A fresh positive balance keeps an otherwise exhausted account eligible.
    pub available_credits_micro_units: Option<u64>,
    #[serde(default)]
    pub provider_credits_available: bool,
    #[serde(default)]
    pub provider_credits_unlimited: bool,
    #[serde(default)]
    pub direct_balance_micro_usd: Option<u64>,
    pub observed_at_ms: u64,
}

/// Provider-neutral result of a quota refresh. Provider adapters populate the
/// normalized window payload and may optionally report whether access is
/// allowed; account state reduction stays independent of the adapter.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaRefreshResult {
    pub quota: QuotaRefreshData,
    pub allowed: Option<bool>,
    /// Optional raw provider signal used to distinguish a blocked account from
    /// an ordinary quota exhaustion. It is intentionally not defaulted to
    /// `false` because providers may omit the signal.
    pub reported_limit_reached: Option<bool>,
}

impl QuotaRefreshData {
    pub fn preserve_subscription_metadata(&mut self, previous_subscription: &Subscription) {
        let Some(subscription) = self.subscription.as_mut() else {
            return;
        };
        if subscription.plan_type.is_none() {
            subscription.plan_type = previous_subscription.plan_type.clone();
        }
        if subscription.active_until_ms.is_none()
            && !subscription_plan_changed(
                previous_subscription.plan_type.as_deref(),
                subscription.plan_type.as_deref(),
            )
        {
            subscription.active_until_ms = previous_subscription.active_until_ms;
        }
    }

    pub fn normalize(
        self,
        previous_snapshot: &QuotaSnapshot,
    ) -> Result<(QuotaSnapshot, Option<Subscription>), QuotaNormalizationError> {
        let primary = normalize_window(
            self.primary,
            QuotaWindowKind::Primary,
            previous_snapshot.primary.as_ref(),
        )?;
        let secondary = normalize_window(
            self.secondary,
            QuotaWindowKind::Secondary,
            previous_snapshot.secondary.as_ref(),
        )?;
        let supplemental =
            normalize_supplemental(self.supplemental, &previous_snapshot.supplemental)?;
        Ok((
            QuotaSnapshot {
                primary,
                secondary,
                supplemental,
                limit_reached: self.limit_reached,
                reset_credits_available: self.reset_credits_available,
                available_credits_micro_units: self.available_credits_micro_units,
                provider_credits_available: self.provider_credits_available,
                provider_credits_unlimited: self.provider_credits_unlimited,
                direct_balance_micro_usd: self.direct_balance_micro_usd,
                updated_at_ms: Some(self.observed_at_ms),
                error: None,
            },
            self.subscription.map(Subscription::normalize),
        ))
    }
}

pub fn subscription_plan_changed(previous_plan: Option<&str>, observed_plan: Option<&str>) -> bool {
    let Some(observed_plan_name) = observed_plan.map(str::trim).filter(|plan| !plan.is_empty())
    else {
        return false;
    };
    previous_plan
        .map(str::trim)
        .filter(|plan| !plan.is_empty())
        .is_none_or(|previous_plan| {
            normalize_subscription_plan(previous_plan)
                != normalize_subscription_plan(observed_plan_name)
        })
}

fn normalize_supplemental(
    inputs: Vec<SupplementalQuotaWindowInput>,
    previous_windows: &[SupplementalQuotaWindow],
) -> Result<Vec<SupplementalQuotaWindow>, QuotaNormalizationError> {
    if inputs.len() > MAX_SUPPLEMENTAL_WINDOWS {
        return Err(QuotaNormalizationError::InvalidSupplementalWindow);
    }
    let mut window_ids = HashSet::with_capacity(inputs.len());
    inputs
        .into_iter()
        .map(|window_input| {
            let quota_window_id = window_input.id.trim();
            let label = window_input.label.trim();
            if quota_window_id.is_empty()
                || quota_window_id.len() > 64
                || !quota_window_id.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.')
                })
                || !window_ids.insert(quota_window_id.to_string())
                || label.is_empty()
                || label.len() > 128
                || label.chars().any(char::is_control)
            {
                return Err(QuotaNormalizationError::InvalidSupplementalWindow);
            }
            let previous_window = previous_windows
                .iter()
                .find(|candidate| candidate.id == quota_window_id)
                .map(|candidate| &candidate.window);
            Ok(SupplementalQuotaWindow {
                id: quota_window_id.to_string(),
                label: label.to_string(),
                service_tier: window_input.service_tier,
                window: QuotaWindow::normalize(window_input.window, previous_window)?,
            })
        })
        .collect()
}

pub trait QuotaAdapter: Send + Sync {
    fn capabilities(&self) -> QuotaAdapterCapabilities;

    fn refresh<'a>(
        &'a self,
        context: &'a QuotaAdapterContext,
        access_token: &'a str,
        now_ms: u64,
    ) -> BoxFuture<'a, Result<QuotaRefreshResult, QuotaRefreshFailure>>;
}

fn normalize_window(
    quota_window_input: Option<QuotaWindowInput>,
    expected: QuotaWindowKind,
    previous_window: Option<&QuotaWindow>,
) -> Result<Option<QuotaWindow>, QuotaNormalizationError> {
    let Some(quota_input) = quota_window_input else {
        return Ok(None);
    };
    if quota_input.kind != expected {
        return Err(QuotaNormalizationError::MismatchedWindowKind);
    }
    let window = QuotaWindow::normalize(quota_input, previous_window)?;
    Ok((!window.is_empty_provider_placeholder()).then_some(window))
}

#[cfg(test)]
mod tests;

use super::windows::normalize_subscription_plan;
use super::{
    QuotaNormalizationError, QuotaSnapshot, QuotaWindow, QuotaWindowInput, QuotaWindowKind,
    Subscription, SubscriptionInput, SupplementalQuotaWindow,
};
use crate::error::{normalize_error_code, safe_error_code};
use crate::DefaultServiceTier;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
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
    pub fn preserve_subscription_metadata(&mut self, previous: &Subscription) {
        let Some(subscription) = self.subscription.as_mut() else {
            return;
        };
        if subscription.plan_type.is_none() {
            subscription.plan_type = previous.plan_type.clone();
        }
        if subscription.active_until_ms.is_none()
            && !subscription_plan_changed(
                previous.plan_type.as_deref(),
                subscription.plan_type.as_deref(),
            )
        {
            subscription.active_until_ms = previous.active_until_ms;
        }
    }

    pub fn normalize(
        self,
        previous: &QuotaSnapshot,
    ) -> Result<(QuotaSnapshot, Option<Subscription>), QuotaNormalizationError> {
        let primary = normalize_window(
            self.primary,
            QuotaWindowKind::Primary,
            previous.primary.as_ref(),
        )?;
        let secondary = normalize_window(
            self.secondary,
            QuotaWindowKind::Secondary,
            previous.secondary.as_ref(),
        )?;
        let supplemental = normalize_supplemental(self.supplemental, &previous.supplemental)?;
        Ok((
            QuotaSnapshot {
                primary,
                secondary,
                supplemental,
                limit_reached: self.limit_reached,
                reset_credits_available: self.reset_credits_available,
                direct_balance_micro_usd: self.direct_balance_micro_usd,
                updated_at_ms: Some(self.observed_at_ms),
                error: None,
            },
            self.subscription.map(Subscription::normalize),
        ))
    }
}

pub fn subscription_plan_changed(previous: Option<&str>, observed: Option<&str>) -> bool {
    let Some(observed) = observed.map(str::trim).filter(|value| !value.is_empty()) else {
        return false;
    };
    previous
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none_or(|previous| {
            normalize_subscription_plan(previous) != normalize_subscription_plan(observed)
        })
}

fn normalize_supplemental(
    inputs: Vec<SupplementalQuotaWindowInput>,
    previous: &[SupplementalQuotaWindow],
) -> Result<Vec<SupplementalQuotaWindow>, QuotaNormalizationError> {
    if inputs.len() > MAX_SUPPLEMENTAL_WINDOWS {
        return Err(QuotaNormalizationError::InvalidSupplementalWindow);
    }
    let mut ids = HashSet::with_capacity(inputs.len());
    inputs
        .into_iter()
        .map(|input| {
            let id = input.id.trim();
            let label = input.label.trim();
            if id.is_empty()
                || id.len() > 64
                || !id.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.')
                })
                || !ids.insert(id.to_string())
                || label.is_empty()
                || label.len() > 128
                || label.chars().any(char::is_control)
            {
                return Err(QuotaNormalizationError::InvalidSupplementalWindow);
            }
            let previous_window = previous
                .iter()
                .find(|candidate| candidate.id == id)
                .map(|candidate| &candidate.window);
            Ok(SupplementalQuotaWindow {
                id: id.to_string(),
                label: label.to_string(),
                service_tier: input.service_tier,
                window: QuotaWindow::normalize(input.window, previous_window)?,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaRefreshFailure {
    pub code: String,
    pub retryable: bool,
    http_status: Option<u16>,
}

impl QuotaRefreshFailure {
    pub fn new(code: &str, retryable: bool) -> Self {
        Self {
            code: safe_error_code(code),
            retryable,
            http_status: None,
        }
    }

    pub fn http_status(&self) -> Option<u16> {
        self.http_status
    }

    pub(crate) fn with_http_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }
}

pub fn classify_quota_http_failure(status: u16, body: &[u8]) -> QuotaRefreshFailure {
    let retryable = status == 429 || status >= 500;
    let code = provider_error_code(body).unwrap_or_else(|| {
        match status {
            401 => "quota_unauthorized",
            403 => "quota_forbidden",
            429 => "quota_rate_limited",
            500..=599 => "quota_upstream",
            _ => "quota_http_status",
        }
        .to_string()
    });
    QuotaRefreshFailure::new(&code, retryable).with_http_status(status)
}

fn provider_error_code(body: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    [
        "/detail/code",
        "/detail/error/code",
        "/error/code",
        "/code",
        "/error/type",
        "/type",
    ]
    .into_iter()
    .filter_map(|pointer| value.pointer(pointer).and_then(serde_json::Value::as_str))
    .find_map(normalize_error_code)
}

fn normalize_window(
    input: Option<QuotaWindowInput>,
    expected: QuotaWindowKind,
    previous: Option<&QuotaWindow>,
) -> Result<Option<QuotaWindow>, QuotaNormalizationError> {
    let Some(input) = input else {
        return Ok(None);
    };
    if input.kind != expected {
        return Err(QuotaNormalizationError::MismatchedWindowKind);
    }
    let window = QuotaWindow::normalize(input, previous)?;
    Ok((!window.is_empty_provider_placeholder()).then_some(window))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(kind: QuotaWindowKind, available_percent: f64) -> QuotaWindowInput {
        QuotaWindowInput {
            kind,
            available_percent: Some(available_percent),
            explicitly_full: None,
            reset: None,
            window_minutes: Some(300),
            provider_cycle_id: None,
            observed_at_ms: 1_000,
        }
    }

    fn supplemental(id: String) -> SupplementalQuotaWindowInput {
        SupplementalQuotaWindowInput {
            id,
            label: " Code Review ".into(),
            service_tier: None,
            window: window(QuotaWindowKind::Primary, 75.0),
        }
    }

    #[test]
    fn supplemental_windows_are_normalized_without_raw_provider_data() {
        let data = QuotaRefreshData {
            supplemental: vec![supplemental("code_review:primary".into())],
            observed_at_ms: 1_000,
            ..Default::default()
        };
        let (snapshot, _) = data.normalize(&QuotaSnapshot::default()).unwrap();
        assert_eq!(snapshot.supplemental[0].id, "code_review:primary");
        assert_eq!(snapshot.supplemental[0].label, "Code Review");
        assert_eq!(
            snapshot.supplemental[0].window.available_basis_points,
            Some(7_500)
        );
    }

    #[test]
    fn supplemental_window_ids_are_unique_bounded_and_safe() {
        let duplicate = QuotaRefreshData {
            supplemental: vec![
                supplemental("code_review:primary".into()),
                supplemental("code_review:primary".into()),
            ],
            ..Default::default()
        };
        assert_eq!(
            duplicate.normalize(&QuotaSnapshot::default()).unwrap_err(),
            QuotaNormalizationError::InvalidSupplementalWindow
        );

        let oversized = QuotaRefreshData {
            supplemental: (0..=MAX_SUPPLEMENTAL_WINDOWS)
                .map(|index| supplemental(format!("additional:{index}")))
                .collect(),
            ..Default::default()
        };
        assert_eq!(
            oversized.normalize(&QuotaSnapshot::default()).unwrap_err(),
            QuotaNormalizationError::InvalidSupplementalWindow
        );

        let unsafe_id = QuotaRefreshData {
            supplemental: vec![supplemental("unsafe id".into())],
            ..Default::default()
        };
        assert_eq!(
            unsafe_id.normalize(&QuotaSnapshot::default()).unwrap_err(),
            QuotaNormalizationError::InvalidSupplementalWindow
        );
    }

    #[test]
    fn quota_refresh_preserves_subscription_expiry_when_usage_only_reports_plan() {
        let previous = Subscription::normalize(SubscriptionInput {
            plan_type: Some("plus".into()),
            active_until_ms: Some(2_000),
            forbidden: false,
            observed_at_ms: 1,
        });
        let mut data = QuotaRefreshData {
            subscription: Some(SubscriptionInput {
                plan_type: Some("plus".into()),
                active_until_ms: None,
                forbidden: false,
                observed_at_ms: 10,
            }),
            ..Default::default()
        };

        data.preserve_subscription_metadata(&previous);

        assert_eq!(data.subscription.unwrap().active_until_ms, Some(2_000));
    }

    #[test]
    fn quota_refresh_preserves_expiry_across_openai_plan_aliases() {
        for (previous_plan, observed_plan) in [
            ("chatgptplusplan", "plus"),
            ("chatgptbusinessplan", "business"),
            ("chatgptteamplan", "business"),
        ] {
            let previous = Subscription::normalize(SubscriptionInput {
                plan_type: Some(previous_plan.into()),
                active_until_ms: Some(2_000),
                forbidden: false,
                observed_at_ms: 1,
            });
            let mut data = QuotaRefreshData {
                subscription: Some(SubscriptionInput {
                    plan_type: Some(observed_plan.into()),
                    active_until_ms: None,
                    forbidden: false,
                    observed_at_ms: 10,
                }),
                ..Default::default()
            };

            data.preserve_subscription_metadata(&previous);

            assert_eq!(data.subscription.unwrap().active_until_ms, Some(2_000));
        }
    }

    #[test]
    fn quota_refresh_does_not_copy_an_expiry_between_different_plans() {
        for (previous_plan, observed_plan) in [
            ("free", "plus"),
            ("plus", "business"),
            ("business", "pro"),
            ("team", "free"),
        ] {
            let previous = Subscription::normalize(SubscriptionInput {
                plan_type: Some(previous_plan.into()),
                active_until_ms: Some(2_000),
                forbidden: false,
                observed_at_ms: 1,
            });
            let mut data = QuotaRefreshData {
                subscription: Some(SubscriptionInput {
                    plan_type: Some(observed_plan.into()),
                    active_until_ms: None,
                    forbidden: false,
                    observed_at_ms: 10,
                }),
                ..Default::default()
            };

            data.preserve_subscription_metadata(&previous);

            assert_eq!(data.subscription.unwrap().active_until_ms, None);
        }
    }

    #[test]
    fn quota_http_failure_keeps_safe_provider_codes() {
        let invalidated =
            classify_quota_http_failure(401, br#"{"detail":{"code":"token_invalidated"}}"#);
        assert_eq!(invalidated.code, "token_invalidated");
        assert_eq!(invalidated.http_status(), Some(401));
        assert!(!invalidated.retryable);

        assert_eq!(
            classify_quota_http_failure(401, b"task expired").code,
            "quota_unauthorized"
        );

        let workspace =
            classify_quota_http_failure(402, br#"{"error":{"code":"deactivated_workspace"}}"#);
        assert_eq!(workspace.code, "deactivated_workspace");
        assert_eq!(workspace.http_status(), Some(402));
        assert!(!workspace.retryable);

        assert_eq!(
            classify_quota_http_failure(401, b"").code,
            "quota_unauthorized"
        );
        assert!(classify_quota_http_failure(503, b"").retryable);
    }
}

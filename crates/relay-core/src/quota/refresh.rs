use super::{
    QuotaNormalizationError, QuotaSnapshot, QuotaWindow, QuotaWindowInput, QuotaWindowKind,
    Subscription, SubscriptionInput, SupplementalQuotaWindow,
};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashSet};

const MAX_SUPPLEMENTAL_WINDOWS: usize = 32;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupplementalQuotaWindowInput {
    pub id: String,
    pub label: String,
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
    pub stable_identity: String,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaRefreshData {
    pub primary: Option<QuotaWindowInput>,
    pub secondary: Option<QuotaWindowInput>,
    #[serde(default)]
    pub supplemental: Vec<SupplementalQuotaWindowInput>,
    pub subscription: Option<SubscriptionInput>,
    pub reset_credits_available: Option<u32>,
    pub observed_at_ms: u64,
}

impl QuotaRefreshData {
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
                reset_credits_available: self.reset_credits_available,
                updated_at_ms: Some(self.observed_at_ms),
                error: None,
            },
            self.subscription.map(Subscription::normalize),
        ))
    }
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
    ) -> BoxFuture<'a, Result<QuotaRefreshData, QuotaRefreshFailure>>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuotaRefreshFailure {
    pub code: String,
    pub retryable: bool,
}

impl QuotaRefreshFailure {
    pub fn new(code: &str, retryable: bool) -> Self {
        Self {
            code: safe_code(code),
            retryable,
        }
    }
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
    QuotaWindow::normalize(input, previous).map(Some)
}

fn safe_code(value: &str) -> String {
    let value = value.trim();
    if !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        value.to_string()
    } else {
        "redacted".to_string()
    }
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
}

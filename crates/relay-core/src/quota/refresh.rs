use super::{
    QuotaNormalizationError, QuotaSnapshot, QuotaWindow, QuotaWindowInput, QuotaWindowKind,
    Subscription, SubscriptionInput,
};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

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
        Ok((
            QuotaSnapshot {
                primary,
                secondary,
                reset_credits_available: self.reset_credits_available,
                updated_at_ms: Some(self.observed_at_ms),
                error: None,
            },
            self.subscription.map(Subscription::normalize),
        ))
    }
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

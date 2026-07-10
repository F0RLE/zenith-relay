use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

const DEFAULT_FULL_THRESHOLD_BASIS_POINTS: u16 = 9_950;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaWindowKind {
    Primary,
    Secondary,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum ResetTime {
    AbsoluteUnixSeconds(u64),
    AbsoluteUnixMilliseconds(u64),
    RelativeSeconds(u64),
}

impl ResetTime {
    pub fn normalize_ms(self, observed_at_ms: u64) -> u64 {
        match self {
            Self::AbsoluteUnixSeconds(seconds) => seconds.saturating_mul(1_000),
            Self::AbsoluteUnixMilliseconds(milliseconds) => milliseconds,
            Self::RelativeSeconds(seconds) => {
                observed_at_ms.saturating_add(seconds.saturating_mul(1_000))
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindowInput {
    pub kind: QuotaWindowKind,
    pub available_percent: Option<f64>,
    pub explicitly_full: Option<bool>,
    pub reset: Option<ResetTime>,
    pub window_minutes: Option<u32>,
    pub provider_cycle_id: Option<String>,
    pub observed_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub kind: QuotaWindowKind,
    pub available_basis_points: Option<u16>,
    pub explicitly_full: Option<bool>,
    pub reset_at_ms: Option<u64>,
    pub window_minutes: Option<u32>,
    pub observed_at_ms: u64,
    pub full_transition_fingerprint: Option<String>,
}

impl QuotaWindow {
    pub fn normalize(
        input: QuotaWindowInput,
        previous: Option<&Self>,
    ) -> Result<Self, QuotaNormalizationError> {
        let available_basis_points = input
            .available_percent
            .map(percent_to_basis_points)
            .transpose()?;
        let fully_available = input.explicitly_full.unwrap_or_else(|| {
            available_basis_points.is_some_and(|value| value >= DEFAULT_FULL_THRESHOLD_BASIS_POINTS)
        });
        let reset_at_ms = input
            .reset
            .map(|reset| reset.normalize_ms(input.observed_at_ms));
        let full_transition_fingerprint = if fully_available {
            previous
                .filter(|previous| previous.is_fully_available())
                .and_then(|previous| previous.full_transition_fingerprint.clone())
                .or_else(|| {
                    Some(transition_fingerprint(
                        input.kind,
                        reset_at_ms,
                        input.window_minutes,
                        input.provider_cycle_id.as_deref(),
                        input.observed_at_ms,
                    ))
                })
        } else {
            None
        };
        Ok(Self {
            kind: input.kind,
            available_basis_points,
            explicitly_full: input.explicitly_full,
            reset_at_ms,
            window_minutes: input.window_minutes,
            observed_at_ms: input.observed_at_ms,
            full_transition_fingerprint,
        })
    }

    pub fn is_fully_available(&self) -> bool {
        self.explicitly_full.unwrap_or_else(|| {
            self.available_basis_points
                .is_some_and(|value| value >= DEFAULT_FULL_THRESHOLD_BASIS_POINTS)
        })
    }

    pub fn full_transition_from(&self, previous: Option<&Self>) -> Option<QuotaTransition> {
        let previous = previous?;
        (previous.kind == self.kind && !previous.is_fully_available() && self.is_fully_available())
            .then(|| QuotaTransition {
                window_kind: self.kind,
                fingerprint: self.full_transition_fingerprint.clone().unwrap_or_else(|| {
                    transition_fingerprint(
                        self.kind,
                        self.reset_at_ms,
                        self.window_minutes,
                        None,
                        self.observed_at_ms,
                    )
                }),
                transitioned_at_ms: self.observed_at_ms,
            })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaTransition {
    pub window_kind: QuotaWindowKind,
    pub fingerprint: String,
    pub transitioned_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionStatus {
    #[default]
    Unknown,
    Active,
    Expired,
    Forbidden,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionInput {
    pub plan_type: Option<String>,
    pub active_until_ms: Option<u64>,
    pub forbidden: bool,
    pub observed_at_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub plan_type: Option<String>,
    pub active_until_ms: Option<u64>,
    pub status: SubscriptionStatus,
    pub updated_at_ms: Option<u64>,
}

impl Subscription {
    pub fn normalize(input: SubscriptionInput) -> Self {
        let plan_type = input
            .plan_type
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let status = if input.forbidden {
            SubscriptionStatus::Forbidden
        } else if input
            .active_until_ms
            .is_some_and(|active_until| active_until <= input.observed_at_ms)
        {
            SubscriptionStatus::Expired
        } else if plan_type.is_some() || input.active_until_ms.is_some() {
            SubscriptionStatus::Active
        } else {
            SubscriptionStatus::Unknown
        };
        Self {
            plan_type,
            active_until_ms: input.active_until_ms,
            status,
            updated_at_ms: Some(input.observed_at_ms),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaErrorState {
    pub code: String,
    pub occurred_at_ms: u64,
}

impl QuotaErrorState {
    pub fn new(code: &str, occurred_at_ms: u64) -> Self {
        Self {
            code: safe_code(code),
            occurred_at_ms,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSnapshot {
    pub primary: Option<QuotaWindow>,
    pub secondary: Option<QuotaWindow>,
    pub reset_credits_available: Option<u32>,
    pub updated_at_ms: Option<u64>,
    pub error: Option<QuotaErrorState>,
}

impl QuotaSnapshot {
    pub fn window(&self, kind: QuotaWindowKind) -> Option<&QuotaWindow> {
        match kind {
            QuotaWindowKind::Primary => self.primary.as_ref(),
            QuotaWindowKind::Secondary => self.secondary.as_ref(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QuotaNormalizationError {
    InvalidPercentage,
    MismatchedWindowKind,
}

impl fmt::Display for QuotaNormalizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPercentage => {
                formatter.write_str("quota percentage must be between 0 and 100")
            }
            Self::MismatchedWindowKind => {
                formatter.write_str("quota window kind does not match its slot")
            }
        }
    }
}

impl std::error::Error for QuotaNormalizationError {}

fn percent_to_basis_points(value: f64) -> Result<u16, QuotaNormalizationError> {
    if !value.is_finite() || !(0.0..=100.0).contains(&value) {
        return Err(QuotaNormalizationError::InvalidPercentage);
    }
    Ok((value * 100.0).round() as u16)
}

fn transition_fingerprint(
    kind: QuotaWindowKind,
    reset_at_ms: Option<u64>,
    window_minutes: Option<u32>,
    provider_cycle_id: Option<&str>,
    observed_at_ms: u64,
) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            format!(
                "{kind:?}\0{}\0{}\0{}\0{observed_at_ms}",
                reset_at_ms.unwrap_or_default(),
                window_minutes.unwrap_or_default(),
                provider_cycle_id.unwrap_or_default().trim(),
            )
            .as_bytes(),
        )
    )
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

    fn input(kind: QuotaWindowKind, percent: f64, observed_at_ms: u64) -> QuotaWindowInput {
        QuotaWindowInput {
            kind,
            available_percent: Some(percent),
            explicitly_full: None,
            reset: Some(ResetTime::RelativeSeconds(60)),
            window_minutes: Some(300),
            provider_cycle_id: None,
            observed_at_ms,
        }
    }

    #[test]
    fn full_transition_and_fingerprint_survive_restart() {
        let previous =
            QuotaWindow::normalize(input(QuotaWindowKind::Primary, 40.0, 1_000), None).unwrap();
        assert_eq!(previous.reset_at_ms, Some(61_000));
        let full = QuotaWindow::normalize(
            input(QuotaWindowKind::Primary, 99.5, 2_000),
            Some(&previous),
        )
        .unwrap();
        let transition = full.full_transition_from(Some(&previous)).unwrap();
        assert_eq!(transition.window_kind, QuotaWindowKind::Primary);

        let serialized = serde_json::to_string(&full).unwrap();
        let restored: QuotaWindow = serde_json::from_str(&serialized).unwrap();
        let still_full = QuotaWindow::normalize(
            input(QuotaWindowKind::Primary, 100.0, 3_000),
            Some(&restored),
        )
        .unwrap();
        assert_eq!(
            still_full.full_transition_fingerprint,
            full.full_transition_fingerprint
        );
        assert!(still_full.full_transition_from(Some(&restored)).is_none());

        let used = QuotaWindow::normalize(
            input(QuotaWindowKind::Primary, 20.0, 4_000),
            Some(&still_full),
        )
        .unwrap();
        let next_full =
            QuotaWindow::normalize(input(QuotaWindowKind::Primary, 100.0, 5_000), Some(&used))
                .unwrap();
        assert_ne!(
            next_full.full_transition_fingerprint,
            full.full_transition_fingerprint
        );
    }

    #[test]
    fn absolute_and_relative_reset_times_normalize_to_epoch_milliseconds() {
        assert_eq!(
            ResetTime::AbsoluteUnixSeconds(20).normalize_ms(1_000),
            20_000
        );
        assert_eq!(
            ResetTime::AbsoluteUnixMilliseconds(20).normalize_ms(1_000),
            20
        );
        assert_eq!(ResetTime::RelativeSeconds(20).normalize_ms(1_000), 21_000);
    }
}

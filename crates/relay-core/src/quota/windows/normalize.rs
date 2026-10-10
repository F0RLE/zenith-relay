use super::{
    QuotaNormalizationError, QuotaTransition, QuotaWindow, QuotaWindowInput, QuotaWindowKind,
    DEFAULT_FULL_THRESHOLD_BASIS_POINTS,
};
use sha2::{Digest, Sha256};

impl QuotaWindow {
    pub fn normalize(
        quota_input: QuotaWindowInput,
        previous_window: Option<&Self>,
    ) -> Result<Self, QuotaNormalizationError> {
        let available_basis_points = quota_input
            .available_percent
            .map(percent_to_basis_points)
            .transpose()?;
        let fully_available = quota_input.explicitly_full.unwrap_or_else(|| {
            available_basis_points
                .is_some_and(|basis_points| basis_points >= DEFAULT_FULL_THRESHOLD_BASIS_POINTS)
        });
        let reset_at_ms = quota_input
            .reset
            .map(|reset| reset.normalize_ms(quota_input.observed_at_ms));
        let full_transition_fingerprint = if fully_available {
            previous_window
                .filter(|previous_window| previous_window.is_fully_available())
                .and_then(|previous_window| previous_window.full_transition_fingerprint.clone())
                .or_else(|| {
                    Some(cycle_fingerprint(
                        false,
                        quota_input.kind,
                        reset_at_ms,
                        quota_input.window_minutes,
                        quota_input.provider_cycle_id.as_deref(),
                        quota_input.observed_at_ms,
                    ))
                })
        } else {
            None
        };
        let exhausted = available_basis_points == Some(0);
        let exhaustion_transition_fingerprint = if exhausted {
            previous_window
                .filter(|previous_window| previous_window.is_exhausted())
                .and_then(|previous_window| {
                    previous_window.exhaustion_transition_fingerprint.clone()
                })
                .or_else(|| {
                    Some(cycle_fingerprint(
                        true,
                        quota_input.kind,
                        reset_at_ms,
                        quota_input.window_minutes,
                        quota_input.provider_cycle_id.as_deref(),
                        quota_input.observed_at_ms,
                    ))
                })
        } else {
            None
        };
        Ok(Self {
            kind: quota_input.kind,
            provider_cycle_id: quota_input.provider_cycle_id,
            window_start_ms: quota_input.window_minutes.and_then(|minutes| {
                reset_at_ms.map(|reset| reset.saturating_sub(u64::from(minutes) * 60_000))
            }),
            available_basis_points,
            explicitly_full: quota_input.explicitly_full,
            reset_at_ms,
            window_minutes: quota_input.window_minutes,
            observed_at_ms: quota_input.observed_at_ms,
            full_transition_fingerprint,
            exhaustion_transition_fingerprint,
        })
    }

    pub fn is_fully_available(&self) -> bool {
        self.explicitly_full.unwrap_or_else(|| {
            self.available_basis_points
                .is_some_and(|basis_points| basis_points >= DEFAULT_FULL_THRESHOLD_BASIS_POINTS)
        })
    }

    pub fn is_exhausted(&self) -> bool {
        self.available_basis_points == Some(0)
    }

    pub(crate) fn is_empty_provider_placeholder(&self) -> bool {
        self.kind == QuotaWindowKind::Secondary
            && self.available_basis_points == Some(10_000)
            && self.window_minutes.unwrap_or_default() == 0
            && self
                .reset_at_ms
                .is_none_or(|reset_at_ms| reset_at_ms <= self.observed_at_ms)
    }

    pub fn full_transition_from(&self, previous_window: Option<&Self>) -> Option<QuotaTransition> {
        let previous_window = previous_window?;
        (previous_window.kind == self.kind
            && !previous_window.is_fully_available()
            && self.is_fully_available())
        .then(|| QuotaTransition {
            window_kind: self.kind,
            fingerprint: self.full_transition_fingerprint.clone().unwrap_or_else(|| {
                cycle_fingerprint(
                    false,
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

    pub fn exhaustion_transition_from(
        &self,
        previous_window: Option<&Self>,
    ) -> Option<QuotaTransition> {
        let previous_window = previous_window?;
        (previous_window.kind == self.kind
            && !previous_window.is_exhausted()
            && self.is_exhausted())
        .then(|| QuotaTransition {
            window_kind: self.kind,
            fingerprint: self
                .exhaustion_transition_fingerprint
                .clone()
                .unwrap_or_else(|| {
                    cycle_fingerprint(
                        true,
                        self.kind,
                        self.reset_at_ms,
                        self.window_minutes,
                        self.provider_cycle_id.as_deref(),
                        self.observed_at_ms,
                    )
                }),
            transitioned_at_ms: self.observed_at_ms,
        })
    }

    /// Returns the stable exhaustion identity for the current window. This is
    /// used by recovery automations when the process starts after the provider
    /// has already reported an exhausted window and no edge transition exists
    /// in the current refresh response.
    pub fn exhaustion_transition(&self) -> Option<QuotaTransition> {
        self.is_exhausted().then(|| QuotaTransition {
            window_kind: self.kind,
            fingerprint: self
                .exhaustion_transition_fingerprint
                .clone()
                .unwrap_or_else(|| {
                    cycle_fingerprint(
                        true,
                        self.kind,
                        self.reset_at_ms,
                        self.window_minutes,
                        self.provider_cycle_id.as_deref(),
                        self.observed_at_ms,
                    )
                }),
            transitioned_at_ms: self.observed_at_ms,
        })
    }
}

fn percent_to_basis_points(percent: f64) -> Result<u16, QuotaNormalizationError> {
    if !percent.is_finite() || !(0.0..=100.0).contains(&percent) {
        return Err(QuotaNormalizationError::InvalidPercentage);
    }
    Ok((percent * 100.0).round() as u16)
}

fn cycle_fingerprint(
    exhausted: bool,
    kind: QuotaWindowKind,
    reset_at_ms: Option<u64>,
    window_minutes: Option<u32>,
    provider_cycle_id: Option<&str>,
    observed_at_ms: u64,
) -> String {
    let identity = format!(
        "{kind:?}\0{}\0{}\0{}\0{observed_at_ms}",
        reset_at_ms.unwrap_or_default(),
        window_minutes.unwrap_or_default(),
        provider_cycle_id.unwrap_or_default().trim(),
    );
    let text = if exhausted {
        format!("exhausted\0{identity}")
    } else {
        identity
    };
    hex::encode(Sha256::digest(text.as_bytes()))
}

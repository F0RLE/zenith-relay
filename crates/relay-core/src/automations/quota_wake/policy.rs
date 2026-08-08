use super::{
    is_safe_id, MAX_OUTPUT_TOKEN_CAP, MAX_VERIFICATION_DELAY_MS, MAX_WAKE_ATTEMPTS,
    MAX_WAKE_JITTER_SECONDS,
};
use crate::accounts::AccountRecord;
use crate::quota::QuotaWindowKind;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const MODEL_RANK_STRIDE: u32 = 4_096;

pub fn model_lightness_rank(model: &str, index: usize) -> u32 {
    let model = model.to_ascii_lowercase();
    let tier = if model.contains("nano") {
        0
    } else if model.contains("mini") {
        1
    } else {
        2
    };
    tier * MODEL_RANK_STRIDE
        + u32::try_from(index)
            .unwrap_or(u32::MAX)
            .min(MODEL_RANK_STRIDE - 1)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "values")]
pub enum AccountSelector {
    AllEligible,
    AccountIds(BTreeSet<String>),
    Tags(BTreeSet<String>),
}

impl AccountSelector {
    pub(super) fn matches(&self, account: &AccountRecord) -> bool {
        match self {
            Self::AllEligible => true,
            Self::AccountIds(ids) => ids.contains(&account.id),
            Self::Tags(tags) => !tags.is_disjoint(&account.tags),
        }
    }

    pub(super) fn is_valid(&self) -> bool {
        match self {
            Self::AllEligible => true,
            Self::AccountIds(ids) | Self::Tags(ids) => !ids.is_empty(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum WakeModelPolicy {
    LightestSupported,
    Explicit(String),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "seconds")]
pub enum WakeTrigger {
    QuotaFull,
    Daily,
    Weekly,
    Interval(u64),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeExecutionPolicy {
    Automatic,
    RequireConfirmation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeTask {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub account_selector: AccountSelector,
    pub window_kinds: BTreeSet<QuotaWindowKind>,
    pub model_policy: WakeModelPolicy,
    pub trigger: WakeTrigger,
    pub fallback_schedule: Option<WakeTrigger>,
    pub execution_policy: WakeExecutionPolicy,
    pub jitter_seconds: u32,
    pub max_attempts_per_cycle: u8,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl WakeTask {
    pub fn validate(&self) -> Result<(), WakeTaskValidationError> {
        if !is_safe_id(&self.id) {
            return Err(WakeTaskValidationError::InvalidId);
        }
        if self.name.trim().is_empty() {
            return Err(WakeTaskValidationError::InvalidName);
        }
        if self.trigger != WakeTrigger::QuotaFull || self.fallback_schedule.is_some() {
            return Err(WakeTaskValidationError::UnsupportedSchedule);
        }
        if !self.account_selector.is_valid() {
            return Err(WakeTaskValidationError::InvalidAccountSelector);
        }
        if self.window_kinds.is_empty() {
            return Err(WakeTaskValidationError::InvalidWindowSelection);
        }
        if matches!(&self.model_policy, WakeModelPolicy::Explicit(id) if id.trim().is_empty()) {
            return Err(WakeTaskValidationError::InvalidModelPolicy);
        }
        if !(1..=MAX_WAKE_ATTEMPTS).contains(&self.max_attempts_per_cycle) {
            return Err(WakeTaskValidationError::InvalidAttemptLimit);
        }
        if self.jitter_seconds > MAX_WAKE_JITTER_SECONDS {
            return Err(WakeTaskValidationError::InvalidJitter);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakeTaskValidationError {
    InvalidId,
    InvalidName,
    UnsupportedSchedule,
    InvalidAccountSelector,
    InvalidWindowSelection,
    InvalidModelPolicy,
    InvalidAttemptLimit,
    InvalidJitter,
    InvalidAdapterPolicy,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeModel {
    pub id: String,
    pub lightness_rank: u32,
    pub wake_capable: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeAdapterPolicy {
    pub windows_requiring_activity: BTreeSet<QuotaWindowKind>,
    pub models: Vec<WakeModel>,
    pub verification_delay_ms: u64,
    pub output_token_cap: u16,
}

impl WakeAdapterPolicy {
    pub(super) fn is_valid(&self) -> bool {
        self.verification_delay_ms <= MAX_VERIFICATION_DELAY_MS
            && (1..=MAX_OUTPUT_TOKEN_CAP).contains(&self.output_token_cap)
    }
}

pub trait WakePolicyAdapter: Send + Sync {
    fn wake_policy(&self, account: &AccountRecord) -> WakeAdapterPolicy;
}

pub(super) fn select_model(policy: &WakeModelPolicy, models: &[WakeModel]) -> Option<String> {
    match policy {
        WakeModelPolicy::LightestSupported => models
            .iter()
            .filter(|model| model.wake_capable && !model.id.trim().is_empty())
            .min_by(|left, right| {
                left.lightness_rank
                    .cmp(&right.lightness_rank)
                    .then_with(|| left.id.cmp(&right.id))
            })
            .map(|model| model.id.clone()),
        WakeModelPolicy::Explicit(id) => models
            .iter()
            .find(|model| model.wake_capable && model.id.eq_ignore_ascii_case(id.trim()))
            .map(|model| model.id.clone()),
    }
}

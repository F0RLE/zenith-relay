//! Deterministic refresh scheduling for account and source observations.
//!
//! The coordinator owns only due times and single-flight/revision fences.  It
//! never performs HTTP itself.  Desktop and server hosts can therefore use the
//! same rules while keeping credentials, cancellation, and provider parsing in
//! their existing owner-local layers.

use std::collections::BTreeMap;

mod coordinator;
pub mod http;
mod limits;
pub mod service;
pub use limits::RefreshLimits;

const ACTIVE_QUOTA_INTERVAL_MS: u64 = 5 * 60 * 1_000;
const IDLE_QUOTA_INTERVAL_MS: u64 = 15 * 60 * 1_000;
const ACTIVE_MODELS_INTERVAL_MS: u64 = 8 * 60 * 60 * 1_000;
const IDLE_MODELS_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;
const ACTIVE_BALANCE_INTERVAL_MS: u64 = 5 * 60 * 1_000;
const IDLE_BALANCE_INTERVAL_MS: u64 = 30 * 60 * 1_000;
const ACTIVE_METADATA_INTERVAL_MS: u64 = 60 * 60 * 1_000;
const IDLE_METADATA_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;
const PRICES_INTERVAL_MS: u64 = 24 * 60 * 60 * 1_000;

/// How long a completed route still counts as active for the faster cadence.
pub const RECENT_ACTIVITY_WINDOW_MS: u64 = 10 * 60 * 1_000;

pub fn recently_active(last_used_at_ms: Option<u64>, now_ms: u64) -> bool {
    last_used_at_ms
        .is_some_and(|at| at <= now_ms && now_ms.saturating_sub(at) < RECENT_ACTIVITY_WINDOW_MS)
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RefreshKind {
    Auth,
    Quota,
    Models,
    Balance,
    Metadata,
    Prices,
}

impl RefreshKind {
    /// Returns the periodic cadence. Auth is intentionally expiry/event driven
    /// and therefore has no polling cadence.
    pub const fn interval_ms(self, active: bool) -> Option<u64> {
        match (self, active) {
            (Self::Auth, _) => None,
            (Self::Quota, true) => Some(ACTIVE_QUOTA_INTERVAL_MS),
            (Self::Quota, false) => Some(IDLE_QUOTA_INTERVAL_MS),
            (Self::Models, true) => Some(ACTIVE_MODELS_INTERVAL_MS),
            (Self::Models, false) => Some(IDLE_MODELS_INTERVAL_MS),
            (Self::Balance, true) => Some(ACTIVE_BALANCE_INTERVAL_MS),
            (Self::Balance, false) => Some(IDLE_BALANCE_INTERVAL_MS),
            (Self::Metadata, true) => Some(ACTIVE_METADATA_INTERVAL_MS),
            (Self::Metadata, false) => Some(IDLE_METADATA_INTERVAL_MS),
            (Self::Prices, _) => Some(PRICES_INTERVAL_MS),
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RefreshIdentity {
    pub member_id: String,
    /// Host-owned monotonic revisions, not hashes or wall-clock timestamps.
    pub auth_revision: u64,
    pub config_revision: u64,
}

impl RefreshIdentity {
    pub fn new(member_id: impl Into<String>, auth_revision: u64, config_revision: u64) -> Self {
        Self {
            member_id: member_id.into(),
            auth_revision,
            config_revision,
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct RefreshKey {
    identity: RefreshIdentity,
    kind: RefreshKind,
}

impl RefreshKey {
    fn member_range(member_id: &str) -> std::ops::RangeInclusive<Self> {
        Self {
            identity: RefreshIdentity::new(member_id, 0, 0),
            kind: RefreshKind::Auth,
        }..=Self {
            identity: RefreshIdentity::new(member_id, u64::MAX, u64::MAX),
            kind: RefreshKind::Prices,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct RefreshJobId(pub u64);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefreshJob {
    pub id: RefreshJobId,
    pub identity: RefreshIdentity,
    pub kind: RefreshKind,
    pub due_at_ms: u64,
    pub manual: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshOutcome {
    /// A valid read, even when the value has not changed.
    Success,
    FailedRetryAt(u64),
    /// Auth returned successfully but did not produce a usable credential.
    NoProgress,
    /// Confirmed by the adapter, not inferred from a transient HTTP failure.
    Unsupported,
}

/// Shortest wait before a failed refresh is attempted again.
pub const REFRESH_FAILURE_RETRY_MS: u64 = 60_000;

impl RefreshOutcome {
    /// Schedules another attempt at least one minute after `now_ms`.
    /// A longer caller delay, such as provider Retry-After, is preserved.
    pub fn retry_after(now_ms: u64, delay_ms: Option<u64>) -> Self {
        let delay = delay_ms.unwrap_or(0).max(REFRESH_FAILURE_RETRY_MS);
        Self::FailedRetryAt(now_ms.saturating_add(delay))
    }
}

/// Balance HTTP failures do not make inference unavailable. Only a confirmed
/// unsupported statistics endpoint removes its periodic monitoring job.
pub fn source_stats_outcome(stats: &crate::SourceProviderStats, now_ms: u64) -> RefreshOutcome {
    if stats.status == crate::SourceStatsStatus::Unsupported {
        RefreshOutcome::Unsupported
    } else if stats.status == crate::SourceStatsStatus::Available && stats.refresh_error.is_none() {
        RefreshOutcome::Success
    } else {
        RefreshOutcome::retry_after(now_ms, None)
    }
}

/// Schedule a quota verification shortly after the provider's reported reset.
/// The stable per-account jitter spreads simultaneous resets without replacing
/// the ordinary cadence or shortening a provider Retry-After floor.
pub fn quota_reset_delay(
    account_id: &str,
    quota: &crate::quota::QuotaSnapshot,
    now_ms: u64,
) -> Option<u64> {
    let jitter = 5_000
        + account_id.bytes().fold(0_u64, |hash, byte| {
            hash.wrapping_mul(16_777_619) ^ u64::from(byte)
        }) % 10_000;
    quota
        .primary
        .iter()
        .chain(quota.secondary.iter())
        .filter_map(|window| window.reset_at_ms)
        .filter(|at| *at > now_ms)
        .map(|at| at.saturating_add(jitter).saturating_sub(now_ms))
        .min()
}

/// Only a recent provider window can replace a quota poll. A carried-forward
/// window from an older read is not new evidence for the whole quota scope.
pub fn passive_quota_age_ms(quota: &crate::quota::QuotaSnapshot, now_ms: u64) -> Option<u64> {
    let updated_at_ms = quota.updated_at_ms?;
    if quota.error.is_some() || updated_at_ms > now_ms {
        return None;
    }
    let mut windows = quota
        .primary
        .iter()
        .chain(quota.secondary.iter())
        .peekable();
    windows.peek()?;
    let (oldest, newest) = windows.try_fold((u64::MAX, 0_u64), |(oldest, newest), window| {
        (window.available_basis_points.is_some() || window.reset_at_ms.is_some()).then_some((
            oldest.min(window.observed_at_ms),
            newest.max(window.observed_at_ms),
        ))
    })?;
    if oldest > now_ms || newest < updated_at_ms {
        return None;
    }
    Some(now_ms.saturating_sub(oldest.min(updated_at_ms)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshFreshness {
    Unknown,
    Fresh { as_of_ms: u64 },
    Stale { as_of_ms: u64 },
    Unsupported,
}

mod source_stats;
pub use source_stats::{SourceStatsObservation, SourceStatsRead};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefreshCompletion {
    Applied { next_due_ms: Option<u64> },
    Stale,
    UnknownJob,
}

#[derive(Clone, Debug)]
struct RefreshEntry {
    origin: String,
    next_due_ms: Option<u64>,
    not_before_ms: u64,
    reschedule_due_ms: Option<u64>,
    event_due_ms: Option<u64>,
    active: bool,
    automatic: bool,
    manual: bool,
    dirty: bool,
    in_flight: Option<RefreshJobId>,
    last_success_ms: Option<u64>,
    passive_observation_wall_ms: Option<u64>,
    passive_age_at_receive: Option<(u64, u64)>,
    passive_fresh_until_ms: Option<u64>,
    passive_during_job_due_ms: Option<u64>,
    failed: bool,
    unsupported: bool,
    no_progress_count: u32,
}

impl RefreshEntry {
    fn new(origin: String, active: bool) -> Self {
        Self {
            origin,
            active,
            automatic: true,
            manual: false,
            next_due_ms: None,
            not_before_ms: 0,
            reschedule_due_ms: None,
            event_due_ms: None,
            dirty: false,
            in_flight: None,
            last_success_ms: None,
            passive_observation_wall_ms: None,
            passive_age_at_receive: None,
            passive_fresh_until_ms: None,
            passive_during_job_due_ms: None,
            failed: false,
            unsupported: false,
            no_progress_count: 0,
        }
    }

    fn schedule(&mut self, due: Option<u64>) {
        if self.unsupported {
            return;
        }
        if self.in_flight.is_some() {
            self.reschedule_due_ms = earliest(self.reschedule_due_ms, due);
        } else {
            self.next_due_ms = earliest(self.next_due_ms, due);
        }
    }
}

#[derive(Clone, Debug)]
struct RunningJob {
    key: RefreshKey,
    origin: String,
}

/// The only due/single-flight/traffic owner. Times supplied here must be from
/// one monotonic runtime clock; persisted provider dates are converted by hosts.
#[derive(Clone, Debug)]
pub struct RefreshCoordinator {
    entries: BTreeMap<RefreshKey, RefreshEntry>,
    jobs: BTreeMap<RefreshJobId, RunningJob>,
    next_job_id: u64,
    limits: RefreshLimits,
    next_start_ms: u64,
    origin_next_start: BTreeMap<String, u64>,
    class_cursor: usize,
}

impl Default for RefreshCoordinator {
    fn default() -> Self {
        Self::new(RefreshLimits::default()).expect("valid refresh defaults")
    }
}

fn earliest(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, None) | (None, left) => left,
    }
}

#[cfg(test)]
mod tests;

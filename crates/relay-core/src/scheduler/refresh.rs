//! Deterministic refresh scheduling for account and source observations.
//!
//! The coordinator owns only due times and single-flight/revision fences.  It
//! never performs HTTP itself.  Desktop and server hosts can therefore use the
//! same rules while keeping credentials, cancellation, and provider parsing in
//! their existing owner-local layers.

use std::collections::BTreeMap;

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

/// Balance HTTP failures do not make inference unavailable. Only a confirmed
/// unsupported statistics endpoint removes its periodic monitoring job.
pub fn source_stats_outcome(stats: &crate::SourceProviderStats, now_ms: u64) -> RefreshOutcome {
    if stats.status == crate::SourceStatsStatus::Unsupported {
        RefreshOutcome::Unsupported
    } else if stats.status == crate::SourceStatsStatus::Available && stats.refresh_error.is_none() {
        RefreshOutcome::Success
    } else {
        RefreshOutcome::FailedRetryAt(now_ms.saturating_add(60_000))
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
pub use source_stats::SourceStatsObservation;

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

impl RefreshCoordinator {
    pub fn new(limits: RefreshLimits) -> Result<Self, &'static str> {
        if limits.max_entries == 0
            || limits.concurrent == 0
            || limits.per_origin == 0
            || limits.reserved_auth >= limits.concurrent
            || limits.reserved_auth >= limits.per_origin
        {
            return Err("refresh limits must leave capacity for ordinary work");
        }
        Ok(Self {
            entries: BTreeMap::new(),
            jobs: BTreeMap::new(),
            next_job_id: 0,
            limits,
            next_start_ms: 0,
            origin_next_start: BTreeMap::new(),
            class_cursor: 0,
        })
    }

    pub fn register(
        &mut self,
        identity: RefreshIdentity,
        kind: RefreshKind,
        now_ms: u64,
        active: bool,
        due_now: bool,
    ) -> bool {
        let origin = identity.member_id.clone();
        self.register_origin(identity, kind, origin, now_ms, active, due_now)
    }

    pub fn register_origin(
        &mut self,
        identity: RefreshIdentity,
        kind: RefreshKind,
        origin: String,
        now_ms: u64,
        active: bool,
        due_now: bool,
    ) -> bool {
        let key = RefreshKey { identity, kind };
        if !self.entries.contains_key(&key) && self.entries.len() >= self.limits.max_entries {
            return false;
        }
        let entry = self
            .entries
            .entry(key)
            .or_insert_with(|| RefreshEntry::new(origin, active));
        entry.active = active;
        let due = if due_now {
            Some(now_ms)
        } else {
            kind.interval_ms(active)
                .map(|interval| now_ms.saturating_add(interval))
        };
        // Registration is reconciliation, not an observation dirty event.
        // Repeated startup/manual registrations join existing work.
        if entry.in_flight.is_none() {
            entry.schedule(due);
        }
        true
    }

    pub fn schedule_at(
        &mut self,
        identity: RefreshIdentity,
        kind: RefreshKind,
        due_at_ms: u64,
        active: bool,
    ) -> bool {
        let key = RefreshKey {
            identity: identity.clone(),
            kind,
        };
        if !self.entries.contains_key(&key)
            && !self.register(identity, kind, due_at_ms, active, false)
        {
            return false;
        }
        let entry = self.entries.get_mut(&key).expect("registered refresh");
        entry.active = active;
        entry.schedule(Some(due_at_ms));
        true
    }

    pub fn set_active(&mut self, identity: &RefreshIdentity, active: bool, now_ms: u64) -> bool {
        let mut changed = false;
        for (key, entry) in self
            .entries
            .range_mut(RefreshKey::member_range(&identity.member_id))
        {
            if &key.identity != identity || entry.active == active {
                continue;
            }
            if key.kind == RefreshKind::Quota && entry.automatic && !entry.unsupported {
                if let Some((received_ms, age_ms)) = entry.passive_age_at_receive {
                    let age_ms = age_ms.saturating_add(now_ms.saturating_sub(received_ms));
                    let interval = RefreshKind::Quota
                        .interval_ms(active)
                        .expect("quota has a cadence");
                    let due_ms = now_ms.saturating_add(interval.saturating_sub(age_ms));
                    entry.passive_fresh_until_ms = Some(due_ms);
                    if entry.in_flight.is_some() {
                        entry.passive_during_job_due_ms = Some(due_ms);
                    } else if !entry.manual {
                        entry.next_due_ms = earliest(entry.event_due_ms, Some(due_ms));
                    }
                    entry.active = active;
                    changed = true;
                    continue;
                }
            }
            // Only the idle -> active transition can accelerate a periodic job.
            if active && !entry.active && entry.automatic {
                entry.schedule(key.kind.interval_ms(true).map(|i| {
                    entry
                        .last_success_ms
                        .unwrap_or(now_ms)
                        .saturating_add(i)
                        .max(now_ms)
                }));
            }
            entry.active = active;
            changed = true;
        }
        changed
    }

    /// Observation changes during a send coalesce into one follow-up. Unlike
    /// dirty events, repeated callers use request_now and only join that send.
    pub fn mark_dirty(
        &mut self,
        identity: &RefreshIdentity,
        kind: RefreshKind,
        now_ms: u64,
    ) -> bool {
        let key = RefreshKey {
            identity: identity.clone(),
            kind,
        };
        let Some(entry) = self.entries.get_mut(&key) else {
            return false;
        };
        if entry.unsupported || !entry.automatic {
            return false;
        }
        if entry.in_flight.is_some() {
            entry.dirty = true;
        } else {
            entry.event_due_ms = earliest(entry.event_due_ms, Some(now_ms));
            entry.schedule(Some(now_ms));
        }
        true
    }

    /// A persisted, fresh inference header can replace the next automatic
    /// quota poll for the same account scope. Manual requests, dirty events and
    /// provider-reported reset checks remain explicit work and are not delayed.
    pub fn observe_passive_quota(
        &mut self,
        identity: &RefreshIdentity,
        now_ms: u64,
        age_ms: u64,
        observed_at_wall_ms: u64,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(&RefreshKey {
            identity: identity.clone(),
            kind: RefreshKind::Quota,
        }) else {
            return false;
        };
        let interval = RefreshKind::Quota
            .interval_ms(entry.active)
            .expect("quota has a cadence");
        if !entry.automatic || entry.unsupported || age_ms >= interval {
            return false;
        }
        if entry
            .passive_observation_wall_ms
            .is_some_and(|previous| previous >= observed_at_wall_ms)
        {
            return false;
        }
        // A service may have just started while the header was observed a few
        // minutes earlier. Do not saturate its due time to service-start + N.
        let due_ms = now_ms.saturating_add(interval.saturating_sub(age_ms));
        entry.passive_observation_wall_ms = Some(observed_at_wall_ms);
        entry.passive_age_at_receive = Some((now_ms, age_ms));
        entry.passive_fresh_until_ms = Some(due_ms);
        entry.last_success_ms = Some(now_ms.saturating_sub(age_ms));
        entry.failed = false;
        if entry.in_flight.is_some() {
            entry.passive_during_job_due_ms = Some(due_ms);
        } else if !entry.manual {
            entry.next_due_ms = earliest(entry.event_due_ms, Some(due_ms));
        }
        true
    }

    pub fn request_now(
        &mut self,
        identity: &RefreshIdentity,
        kind: RefreshKind,
        now_ms: u64,
    ) -> bool {
        let key = RefreshKey {
            identity: identity.clone(),
            kind,
        };
        let Some(entry) = self.entries.get_mut(&key) else {
            return false;
        };
        // Explicit manual recheck may retry an unsupported kind, never bypass a hint.
        entry.unsupported = false;
        if entry.in_flight.is_none() {
            entry.manual = true;
            entry.schedule(Some(now_ms));
        }
        true
    }

    /// Record the provider floor before host persistence/finalization, which
    /// can itself fail or be superseded by a newer passive observation.
    pub fn defer_until(&mut self, identity: &RefreshIdentity, kind: RefreshKind, until_ms: u64) {
        if let Some(entry) = self.entries.get_mut(&RefreshKey {
            identity: identity.clone(),
            kind,
        }) {
            entry.not_before_ms = entry.not_before_ms.max(until_ms);
        }
    }

    pub fn claim_due(&mut self, now_ms: u64) -> Option<RefreshJob> {
        let key = self
            .entries
            .iter()
            .filter(|(key, entry)| {
                entry.in_flight.is_none()
                    && !entry.unsupported
                    && self.capacity_available(entry, key.kind)
                    && self.eligible_at(entry).is_some_and(|at| at <= now_ms)
            })
            .min_by_key(|(key, entry)| (self.class_distance(key.kind), entry.next_due_ms, *key))
            .map(|(key, _)| key.clone())?;
        self.next_job_id = self.next_job_id.checked_add(1)?;
        self.advance_class(key.kind);
        let id = RefreshJobId(self.next_job_id);
        let entry = self.entries.get_mut(&key)?;
        let due_at_ms = entry
            .next_due_ms
            .take()
            .unwrap_or(now_ms)
            .max(entry.not_before_ms);
        if entry.event_due_ms.is_some_and(|at| at <= now_ms) {
            entry.event_due_ms = None;
        } else {
            entry.reschedule_due_ms = earliest(entry.reschedule_due_ms, entry.event_due_ms);
        }
        entry.dirty = false;
        entry.in_flight = Some(id);
        self.next_start_ms = now_ms.saturating_add(self.limits.start_spacing_ms);
        self.origin_next_start.insert(
            entry.origin.clone(),
            now_ms.saturating_add(self.limits.origin_spacing_ms),
        );
        self.jobs.insert(
            id,
            RunningJob {
                key: key.clone(),
                origin: entry.origin.clone(),
            },
        );
        Some(RefreshJob {
            id,
            identity: key.identity,
            kind: key.kind,
            due_at_ms,
            manual: std::mem::take(&mut entry.manual),
        })
    }

    pub fn complete(
        &mut self,
        job: &RefreshJob,
        outcome: RefreshOutcome,
        now_ms: u64,
    ) -> RefreshCompletion {
        let Some(running) = self.jobs.get(&job.id) else {
            return RefreshCompletion::UnknownJob;
        };
        if running.key.identity != job.identity || running.key.kind != job.kind {
            // An invalid completion must not steal capacity from the real job.
            return RefreshCompletion::Stale;
        }
        let key = self.jobs.remove(&job.id).expect("known job").key;
        self.prune_origins(now_ms);
        let Some(entry) = self.entries.get_mut(&key) else {
            return RefreshCompletion::Stale;
        };
        if entry.in_flight != Some(job.id) {
            return RefreshCompletion::Stale;
        }
        entry.in_flight = None;
        let rescheduled = entry.reschedule_due_ms.take();
        let passive_due = entry.passive_during_job_due_ms.take();
        let passive_replaced_failure = passive_due.is_some()
            && !entry.dirty
            && !matches!(
                outcome,
                RefreshOutcome::Success | RefreshOutcome::Unsupported
            );
        entry.not_before_ms = entry
            .not_before_ms
            .max(now_ms.saturating_add(self.limits.minimum_interval_ms));
        entry.failed = outcome != RefreshOutcome::Success && !passive_replaced_failure;
        let next = match outcome {
            RefreshOutcome::Success => {
                entry.no_progress_count = 0;
                let periodic = if let Some(due) = passive_due {
                    // A read started before this newer persisted inference
                    // header. Host reducers will discard that older HTTP
                    // result; do not shift the quota due time to job end.
                    Some(due)
                } else {
                    entry.last_success_ms = Some(now_ms);
                    entry.passive_age_at_receive = None;
                    entry.passive_fresh_until_ms = None;
                    key.kind
                        .interval_ms(entry.active)
                        .map(|i| now_ms.saturating_add(i))
                }
                .filter(|_| entry.automatic);
                let scheduled = earliest(periodic, rescheduled);
                if entry.dirty {
                    earliest(scheduled, Some(now_ms))
                } else {
                    scheduled
                }
            }
            RefreshOutcome::FailedRetryAt(at) => {
                entry.not_before_ms = entry.not_before_ms.max(at);
                if passive_replaced_failure {
                    entry.no_progress_count = 0;
                    earliest(passive_due, rescheduled)
                } else {
                    Some(entry.not_before_ms)
                }
            }
            RefreshOutcome::NoProgress => {
                if passive_replaced_failure {
                    entry.no_progress_count = 0;
                    earliest(passive_due, rescheduled)
                } else {
                    entry.no_progress_count = entry.no_progress_count.saturating_add(1);
                    let delay =
                        (5_000u64 << entry.no_progress_count.saturating_sub(1).min(6)).min(300_000);
                    entry.not_before_ms = entry.not_before_ms.max(now_ms.saturating_add(delay));
                    Some(entry.not_before_ms)
                }
            }
            RefreshOutcome::Unsupported => {
                entry.unsupported = true;
                entry.event_due_ms = None;
                None
            }
        };
        entry.dirty = false;
        entry.next_due_ms = next
            .filter(|_| entry.automatic)
            .map(|at| at.max(entry.not_before_ms));
        RefreshCompletion::Applied {
            next_due_ms: entry.next_due_ms,
        }
    }

    pub fn invalidate(&mut self, identity: &RefreshIdentity) -> usize {
        let before = self.entries.len();
        self.entries.retain(|key, _| &key.identity != identity);
        // Running old revisions continue charging capacity until they settle.
        before.saturating_sub(self.entries.len())
    }

    pub fn remove_kind(&mut self, identity: &RefreshIdentity, kind: RefreshKind) -> bool {
        self.entries
            .remove(&RefreshKey {
                identity: identity.clone(),
                kind,
            })
            .is_some()
    }

    pub fn set_automatic(
        &mut self,
        identity: &RefreshIdentity,
        kind: RefreshKind,
        automatic: bool,
    ) -> bool {
        if let Some(entry) = self.entries.get_mut(&RefreshKey {
            identity: identity.clone(),
            kind,
        }) {
            let activated = !entry.automatic && automatic;
            entry.automatic = automatic;
            if !automatic {
                entry.event_due_ms = None;
                if entry.in_flight.is_none() {
                    entry.next_due_ms = None;
                }
            }
            activated
        } else {
            false
        }
    }

    pub fn in_flight(&self, identity: &RefreshIdentity, kind: RefreshKind) -> bool {
        self.entries
            .get(&RefreshKey {
                identity: identity.clone(),
                kind,
            })
            .is_some_and(|entry| entry.in_flight.is_some())
    }

    pub fn schedule_event(
        &mut self,
        identity: &RefreshIdentity,
        kind: RefreshKind,
        at_ms: u64,
    ) -> bool {
        let Some(entry) = self.entries.get_mut(&RefreshKey {
            identity: identity.clone(),
            kind,
        }) else {
            return false;
        };
        if !entry.automatic || entry.unsupported {
            return false;
        }
        entry.event_due_ms = earliest(entry.event_due_ms, Some(at_ms));
        entry.schedule(Some(at_ms));
        true
    }

    pub fn pending_jobs(&self) -> usize {
        self.jobs.len()
    }

    pub fn next_due(&self, identity: &RefreshIdentity, kind: RefreshKind) -> Option<u64> {
        self.entries
            .get(&RefreshKey {
                identity: identity.clone(),
                kind,
            })
            .and_then(|entry| entry.next_due_ms.map(|due| due.max(entry.not_before_ms)))
    }

    /// None when only a running job can unblock dispatch. Callers must wait on
    /// completion/config events rather than spin on an overdue blocked entry.
    pub fn next_wake(&self) -> Option<u64> {
        self.entries
            .iter()
            .filter(|(key, entry)| {
                entry.in_flight.is_none()
                    && !entry.unsupported
                    && self.capacity_available(entry, key.kind)
            })
            .filter_map(|(_, entry)| self.eligible_at(entry))
            .min()
    }

    pub fn freshness(
        &self,
        identity: &RefreshIdentity,
        kind: RefreshKind,
        now_ms: u64,
    ) -> RefreshFreshness {
        let Some(entry) = self.entries.get(&RefreshKey {
            identity: identity.clone(),
            kind,
        }) else {
            return RefreshFreshness::Unknown;
        };
        if entry.unsupported {
            return RefreshFreshness::Unsupported;
        }
        let Some(as_of_ms) = entry.last_success_ms else {
            return RefreshFreshness::Unknown;
        };
        if entry.failed
            || entry
                .passive_fresh_until_ms
                .is_some_and(|until| now_ms >= until)
            || kind
                .interval_ms(entry.active)
                .is_some_and(|i| now_ms >= as_of_ms.saturating_add(i))
        {
            RefreshFreshness::Stale { as_of_ms }
        } else {
            RefreshFreshness::Fresh { as_of_ms }
        }
    }

    fn prune_origins(&mut self, now_ms: u64) {
        let retained = self
            .entries
            .values()
            .map(|entry| entry.origin.as_str())
            .chain(self.jobs.values().map(|job| job.origin.as_str()))
            .collect::<std::collections::BTreeSet<_>>();
        self.origin_next_start
            .retain(|origin, at| *at > now_ms || retained.contains(origin.as_str()));
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

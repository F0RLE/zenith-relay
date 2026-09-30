use super::*;

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
}

mod run;

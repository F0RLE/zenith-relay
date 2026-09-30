use super::*;

impl<T: Send + Sync + 'static> RefreshService<T> {
    pub fn mark_dirty(&self, identity: &RefreshIdentity, kind: RefreshKind) -> bool {
        let changed = self
            .state
            .lock()
            .expect("refresh state poisoned")
            .coordinator
            .mark_dirty(identity, kind, self.now_ms());
        if changed {
            self.signal();
        }
        changed
    }

    /// Host calls this only after a validated passive quota snapshot was
    /// persisted. The age is measured from the oldest supplied quota window.
    pub fn observe_passive_quota(
        &self,
        identity: &RefreshIdentity,
        age_ms: u64,
        observed_at_wall_ms: u64,
    ) -> bool {
        let observed = self
            .state
            .lock()
            .expect("refresh state poisoned")
            .coordinator
            .observe_passive_quota(identity, self.now_ms(), age_ms, observed_at_wall_ms);
        if observed {
            self.signal();
        }
        observed
    }

    pub fn set_active(&self, identity: &RefreshIdentity, active: bool) {
        let changed = self
            .state
            .lock()
            .expect("refresh state poisoned")
            .coordinator
            .set_active(identity, active, self.now_ms());
        if changed {
            self.signal();
        }
    }

    /// A host event may accelerate a registered automatic job, never bypass a
    /// provider floor or revive an unsupported/disabled resource kind.
    pub fn schedule_after(
        &self,
        identity: &RefreshIdentity,
        kind: RefreshKind,
        delay_ms: u64,
    ) -> bool {
        let mut state = self.state.lock().expect("refresh state poisoned");
        let scheduled = state.coordinator.schedule_event(
            identity,
            kind,
            self.now_ms().saturating_add(delay_ms),
        );
        drop(state);
        self.signal();
        scheduled
    }

    pub fn in_flight(&self, identity: &RefreshIdentity, kind: RefreshKind) -> bool {
        self.state
            .lock()
            .expect("refresh state poisoned")
            .coordinator
            .in_flight(identity, kind)
    }

    pub fn remove_member(&self, member_id: &str) -> bool {
        let mut state = self.state.lock().expect("refresh state poisoned");
        let keys = state
            .entries
            .range(RefreshKey::member_range(member_id))
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in &keys {
            Self::remove_entry(&mut state, key);
        }
        drop(state);
        self.signal();
        if !keys.is_empty() {
            self.notify_progress();
        }
        !keys.is_empty()
    }

    /// Retire one resource after its own endpoint changes. A different kind's
    /// in-flight read still charges capacity until it actually finishes.
    pub fn remove_kind(&self, identity: &RefreshIdentity, kind: RefreshKind) -> bool {
        let mut state = self.state.lock().expect("refresh state poisoned");
        let key = RefreshKey {
            identity: identity.clone(),
            kind,
        };
        if !state.entries.contains_key(&key) {
            return false;
        }
        Self::remove_entry(&mut state, &key);
        drop(state);
        self.signal();
        self.notify_progress();
        true
    }

    pub fn respect_retry_after(
        &self,
        identity: &RefreshIdentity,
        kind: RefreshKind,
        delay_ms: u64,
    ) {
        self.state
            .lock()
            .expect("refresh state poisoned")
            .coordinator
            .defer_until(identity, kind, self.now_ms().saturating_add(delay_ms));
        self.signal();
    }

    pub fn set_member_active(&self, member_id: &str) {
        let mut state = self.state.lock().expect("refresh state poisoned");
        let identities = state
            .entries
            .range(RefreshKey::member_range(member_id))
            .map(|(key, _)| key)
            .map(|key| key.identity.clone())
            .collect::<std::collections::BTreeSet<_>>();
        let mut changed = false;
        for identity in identities {
            changed |= state.coordinator.set_active(&identity, true, self.now_ms());
        }
        drop(state);
        if changed {
            self.signal();
        }
    }

    pub fn retain(&self, mut keep: impl FnMut(&RefreshIdentity, RefreshKind) -> bool) {
        let mut state = self.state.lock().expect("refresh state poisoned");
        let removed = state
            .entries
            .keys()
            .filter(|key| !keep(&key.identity, key.kind))
            .cloned()
            .collect::<Vec<_>>();
        for key in removed {
            Self::remove_entry(&mut state, &key);
        }
        drop(state);
        self.signal();
    }
}

use super::*;

impl<T: Send + Sync + 'static> RefreshService<T> {
    pub fn register<F>(
        self: &Arc<Self>,
        registration: RefreshRegistration,
        work: F,
    ) -> Result<(), RefreshWaitError>
    where
        F: Fn(RefreshJob) -> BoxFuture<'static, RefreshResult<T>> + Send + Sync + 'static,
    {
        let mut state = self.state.lock().expect("refresh state poisoned");
        if state.stopped {
            return Err(RefreshWaitError::Stopped);
        }
        let key = RefreshKey {
            identity: registration.identity,
            kind: registration.kind,
        };
        // A caller may have captured its snapshot before a concurrent edit.
        // Its late registration must not evict an already installed revision.
        if state
            .entries
            .range(RefreshKey::member_range(&key.identity.member_id))
            .any(|(current, _)| {
                current.kind == key.kind
                    && current.identity.member_id == key.identity.member_id
                    && (current.identity.auth_revision > key.identity.auth_revision
                        || current.identity.config_revision > key.identity.config_revision)
            })
        {
            return Err(RefreshWaitError::Stale);
        }
        // Configuration reconciliation replaces obsolete queued revisions, but
        // their running HTTP still charges capacity until it actually finishes.
        let obsolete = state
            .entries
            .range(RefreshKey::member_range(&key.identity.member_id))
            .map(|(key, _)| key)
            .filter(|old| {
                old.kind == key.kind
                    && old.identity.member_id == key.identity.member_id
                    && *old != &key
            })
            .cloned()
            .collect::<Vec<_>>();
        for old in obsolete {
            Self::remove_entry(&mut state, &old);
        }
        let new = !state.entries.contains_key(&key);
        state
            .coordinator
            .set_active(&key.identity, registration.active, self.now_ms());
        if !state.coordinator.register_origin(
            key.identity.clone(),
            key.kind,
            registration.origin,
            self.now_ms(),
            registration.active,
            new && registration.due_now && registration.automatic,
        ) {
            return Err(RefreshWaitError::Full);
        }
        // Do not clear an already pending manual request during reconciliation.
        let pending = state
            .entries
            .get(&key)
            .is_some_and(|entry| entry.result.is_some());
        let activated =
            state
                .coordinator
                .set_automatic(&key.identity, key.kind, registration.automatic);
        if activated && registration.due_now {
            state
                .coordinator
                .schedule_event(&key.identity, key.kind, self.now_ms());
        }
        if pending {
            state
                .coordinator
                .request_now(&key.identity, key.kind, self.now_ms());
        }
        let work: Work<T> = Arc::new(work);
        state
            .entries
            .entry(key)
            .and_modify(|entry| entry.work = work.clone())
            .or_insert(Entry {
                work,
                result: None,
                cached: None,
            });
        drop(state);
        self.start();
        self.signal();
        Ok(())
    }
}

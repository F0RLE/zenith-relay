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
        let mut refresh_state = self.state.lock().expect("refresh state poisoned");
        if refresh_state.stopped {
            return Err(RefreshWaitError::Stopped);
        }
        let key = RefreshKey {
            identity: registration.identity,
            kind: registration.kind,
        };
        // A caller may have captured its snapshot before a concurrent edit.
        // Its late registration must not evict an already installed revision.
        if refresh_state
            .entries
            .range(RefreshKey::member_range(&key.identity.member_id))
            .any(|(registered_entry, _)| {
                registered_entry.kind == key.kind
                    && registered_entry.identity.member_id == key.identity.member_id
                    && (registered_entry.identity.auth_revision > key.identity.auth_revision
                        || registered_entry.identity.config_revision > key.identity.config_revision)
            })
        {
            return Err(RefreshWaitError::Stale);
        }
        // Configuration reconciliation replaces obsolete queued revisions, but
        // their running HTTP still charges capacity until it actually finishes.
        let obsolete = refresh_state
            .entries
            .range(RefreshKey::member_range(&key.identity.member_id))
            .map(|(key, _)| key)
            .filter(|obsolete_key| {
                obsolete_key.kind == key.kind
                    && obsolete_key.identity.member_id == key.identity.member_id
                    && *obsolete_key != &key
            })
            .cloned()
            .collect::<Vec<_>>();
        for obsolete_key in obsolete {
            Self::remove_entry(&mut refresh_state, &obsolete_key);
        }
        let is_new = !refresh_state.entries.contains_key(&key);
        refresh_state
            .coordinator
            .set_active(&key.identity, registration.active, self.now_ms());
        if !refresh_state.coordinator.register_origin(
            key.identity.clone(),
            key.kind,
            registration.origin,
            self.now_ms(),
            registration.active,
            is_new && registration.due_now && registration.automatic,
        ) {
            return Err(RefreshWaitError::Full);
        }
        // Do not clear an already pending manual request during reconciliation.
        let has_pending_result = refresh_state
            .entries
            .get(&key)
            .is_some_and(|refresh_entry| refresh_entry.completion_sender.is_some());
        let activated = refresh_state.coordinator.set_automatic(
            &key.identity,
            key.kind,
            registration.automatic,
        );
        if activated && registration.due_now {
            refresh_state
                .coordinator
                .schedule_event(&key.identity, key.kind, self.now_ms());
        }
        if has_pending_result {
            refresh_state
                .coordinator
                .request_now(&key.identity, key.kind, self.now_ms());
        }
        let work: Work<T> = Arc::new(work);
        refresh_state
            .entries
            .entry(key)
            .and_modify(|refresh_entry| refresh_entry.work = work.clone())
            .or_insert(Entry {
                work,
                completion_sender: None,
                cached: None,
            });
        drop(refresh_state);
        self.start();
        self.signal();
        Ok(())
    }
}

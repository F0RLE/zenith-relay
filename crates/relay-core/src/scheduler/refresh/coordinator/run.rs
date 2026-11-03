use super::*;

impl RefreshCoordinator {
    pub fn claim_due(&mut self, now_ms: u64) -> Option<RefreshJob> {
        let key = self
            .entries
            .iter()
            .filter(|(key, refresh_entry)| {
                refresh_entry.in_flight.is_none()
                    && !refresh_entry.unsupported
                    && self.capacity_available(refresh_entry, key.kind)
                    && self
                        .eligible_at(refresh_entry)
                        .is_some_and(|at| at <= now_ms)
            })
            .min_by_key(|(key, refresh_entry)| {
                (
                    self.class_distance(key.kind),
                    refresh_entry.next_due_ms,
                    *key,
                )
            })
            .map(|(key, _)| key.clone())?;
        self.next_job_id = self.next_job_id.checked_add(1)?;
        self.advance_class(key.kind);
        let refresh_job_id = RefreshJobId(self.next_job_id);
        let refresh_entry = self.entries.get_mut(&key)?;
        let due_at_ms = refresh_entry
            .next_due_ms
            .take()
            .unwrap_or(now_ms)
            .max(refresh_entry.not_before_ms);
        if refresh_entry.event_due_ms.is_some_and(|at| at <= now_ms) {
            refresh_entry.event_due_ms = None;
        } else {
            refresh_entry.reschedule_due_ms =
                earliest(refresh_entry.reschedule_due_ms, refresh_entry.event_due_ms);
        }
        refresh_entry.dirty = false;
        refresh_entry.in_flight = Some(refresh_job_id);
        self.next_start_ms = now_ms.saturating_add(self.limits.start_spacing_ms);
        self.origin_next_start.insert(
            refresh_entry.origin.clone(),
            now_ms.saturating_add(self.limits.origin_spacing_ms),
        );
        self.jobs.insert(
            refresh_job_id,
            RunningJob {
                key: key.clone(),
                origin: refresh_entry.origin.clone(),
            },
        );
        Some(RefreshJob {
            id: refresh_job_id,
            identity: key.identity,
            kind: key.kind,
            due_at_ms,
            manual: std::mem::take(&mut refresh_entry.manual),
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
        let Some(refresh_entry) = self.entries.get_mut(&key) else {
            return RefreshCompletion::Stale;
        };
        if refresh_entry.in_flight != Some(job.id) {
            return RefreshCompletion::Stale;
        }
        refresh_entry.in_flight = None;
        let rescheduled = refresh_entry.reschedule_due_ms.take();
        let passive_due = refresh_entry.passive_during_job_due_ms.take();
        let passive_replaced_failure = passive_due.is_some()
            && !refresh_entry.dirty
            && !matches!(
                outcome,
                RefreshOutcome::Success | RefreshOutcome::Unsupported
            );
        refresh_entry.not_before_ms = refresh_entry
            .not_before_ms
            .max(now_ms.saturating_add(self.limits.minimum_interval_ms));
        refresh_entry.failed = outcome != RefreshOutcome::Success && !passive_replaced_failure;
        let next_due_at_ms = match outcome {
            RefreshOutcome::Success => {
                refresh_entry.no_progress_count = 0;
                let periodic = if let Some(due) = passive_due {
                    // A read started before this newer persisted inference
                    // header. Host reducers will discard that older HTTP
                    // result; do not shift the quota due time to job end.
                    Some(due)
                } else {
                    refresh_entry.last_success_ms = Some(now_ms);
                    refresh_entry.passive_age_at_receive = None;
                    refresh_entry.passive_fresh_until_ms = None;
                    key.kind
                        .interval_ms(refresh_entry.active)
                        .map(|i| now_ms.saturating_add(i))
                }
                .filter(|_| refresh_entry.automatic);
                let scheduled = earliest(periodic, rescheduled);
                if refresh_entry.dirty {
                    earliest(scheduled, Some(now_ms))
                } else {
                    scheduled
                }
            }
            RefreshOutcome::FailedRetryAt(at) => {
                refresh_entry.not_before_ms = refresh_entry.not_before_ms.max(at);
                if passive_replaced_failure {
                    refresh_entry.no_progress_count = 0;
                    earliest(passive_due, rescheduled)
                } else {
                    Some(refresh_entry.not_before_ms)
                }
            }
            RefreshOutcome::NoProgress => {
                if passive_replaced_failure {
                    refresh_entry.no_progress_count = 0;
                    earliest(passive_due, rescheduled)
                } else {
                    refresh_entry.no_progress_count =
                        refresh_entry.no_progress_count.saturating_add(1);
                    let delay = (5_000u64
                        << refresh_entry.no_progress_count.saturating_sub(1).min(6))
                    .min(300_000);
                    refresh_entry.not_before_ms = refresh_entry
                        .not_before_ms
                        .max(now_ms.saturating_add(delay));
                    Some(refresh_entry.not_before_ms)
                }
            }
            RefreshOutcome::Unsupported => {
                refresh_entry.unsupported = true;
                refresh_entry.event_due_ms = None;
                None
            }
        };
        refresh_entry.dirty = false;
        refresh_entry.next_due_ms = next_due_at_ms
            .filter(|_| refresh_entry.automatic)
            .map(|at| at.max(refresh_entry.not_before_ms));
        RefreshCompletion::Applied {
            next_due_ms: refresh_entry.next_due_ms,
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
        if let Some(refresh_entry) = self.entries.get_mut(&RefreshKey {
            identity: identity.clone(),
            kind,
        }) {
            let activated = !refresh_entry.automatic && automatic;
            refresh_entry.automatic = automatic;
            if !automatic {
                refresh_entry.event_due_ms = None;
                if refresh_entry.in_flight.is_none() {
                    refresh_entry.next_due_ms = None;
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
            .is_some_and(|refresh_entry| refresh_entry.in_flight.is_some())
    }

    pub fn schedule_event(
        &mut self,
        identity: &RefreshIdentity,
        kind: RefreshKind,
        at_ms: u64,
    ) -> bool {
        let Some(refresh_entry) = self.entries.get_mut(&RefreshKey {
            identity: identity.clone(),
            kind,
        }) else {
            return false;
        };
        if !refresh_entry.automatic || refresh_entry.unsupported {
            return false;
        }
        refresh_entry.event_due_ms = earliest(refresh_entry.event_due_ms, Some(at_ms));
        refresh_entry.schedule(Some(at_ms));
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
            .and_then(|refresh_entry| {
                refresh_entry
                    .next_due_ms
                    .map(|due| due.max(refresh_entry.not_before_ms))
            })
    }

    /// None when only a running job can unblock dispatch. Callers must wait on
    /// completion/config events rather than spin on an overdue blocked entry.
    pub fn next_wake(&self) -> Option<u64> {
        self.entries
            .iter()
            .filter(|(key, refresh_entry)| {
                refresh_entry.in_flight.is_none()
                    && !refresh_entry.unsupported
                    && self.capacity_available(refresh_entry, key.kind)
            })
            .filter_map(|(_, refresh_entry)| self.eligible_at(refresh_entry))
            .min()
    }

    pub fn freshness(
        &self,
        identity: &RefreshIdentity,
        kind: RefreshKind,
        now_ms: u64,
    ) -> RefreshFreshness {
        let Some(refresh_entry) = self.entries.get(&RefreshKey {
            identity: identity.clone(),
            kind,
        }) else {
            return RefreshFreshness::Unknown;
        };
        if refresh_entry.unsupported {
            return RefreshFreshness::Unsupported;
        }
        let Some(as_of_ms) = refresh_entry.last_success_ms else {
            return RefreshFreshness::Unknown;
        };
        if refresh_entry.failed
            || refresh_entry
                .passive_fresh_until_ms
                .is_some_and(|until| now_ms >= until)
            || kind
                .interval_ms(refresh_entry.active)
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
            .map(|refresh_entry| refresh_entry.origin.as_str())
            .chain(self.jobs.values().map(|job| job.origin.as_str()))
            .collect::<std::collections::BTreeSet<_>>();
        self.origin_next_start
            .retain(|origin, at| *at > now_ms || retained.contains(origin.as_str()));
    }
}

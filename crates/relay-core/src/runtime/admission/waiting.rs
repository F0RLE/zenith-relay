use super::*;
use tokio::time::{sleep_until, Instant};

impl GatewayRuntime {
    pub(in crate::runtime) async fn admit(
        &self,
        request: AdmissionRequest,
        now_ms: u64,
    ) -> Option<(Selection, CandidateLease)> {
        let started = Instant::now();
        let mut waiter = None;
        loop {
            // Subscribe before inspecting capacity: release/config events in
            // the inspection-to-await gap must never be lost.
            let notified = self.candidate_availability.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let turn_changed = self.admission_changed.notified();
            tokio::pin!(turn_changed);
            turn_changed.as_mut().enable();
            let deadline = self.admission_deadline(&request.budget).ok()?;
            if !request.budget.can_dispatch() {
                return None;
            }
            let now_ms = now_ms.saturating_add(started.elapsed().as_millis() as u64);
            let reserved = {
                // Queue -> budget/scope -> scheduler is the only admission
                // lock order. Never invoke a host callback under this lock.
                let mut queue = self
                    .admission
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // When every physical capacity is occupied, scanning every
                // queued waiter cannot find a ready turn. Check the incoming
                // route under the same scheduler lock: unsupported requests
                // must still fail immediately instead of joining the queue.
                // Subscribe above before this check, so a racing release is
                // observed even if it happens before waiter registration.
                let saturated_busy = if queue.waiters.is_empty() {
                    None
                } else {
                    let scope = request.key.scope_read();
                    let mut scheduler = self.lock_scheduler();
                    scheduler.all_capacity_reserved().then(|| {
                        scheduler.capacity_blocked_for(
                            request.selection(&scope, now_ms),
                            request.operation,
                        )
                    })
                };
                let turn = if saturated_busy.is_some() {
                    None
                } else {
                    queue.turn(self, now_ms)
                };
                let (reserved, busy) = if let Some(busy) = saturated_busy {
                    (None, busy)
                } else if turn.is_none_or(|id| id == request.budget.request_id()) {
                    self.try_reserve_admission(&request, now_ms)
                } else {
                    let scope = request.key.scope_read();
                    let mut scheduler = self.lock_scheduler();
                    (
                        None,
                        scheduler.admission_ready_for(
                            request.selection(&scope, now_ms),
                            request.operation,
                        ) || scheduler.capacity_blocked_for(
                            request.selection(&scope, now_ms),
                            request.operation,
                        ),
                    )
                };
                if reserved.is_some() {
                    queue.served(&request.key.id);
                } else if !busy {
                    return None;
                } else if waiter.is_none() {
                    waiter = Some(self.register_waiter(&mut queue, &request, true)?);
                }
                reserved
            };
            if let Some((selection, lease)) = reserved {
                drop(waiter);
                self.admission_activity(&selection, &request, now_ms);
                return Some((selection, lease));
            }
            let due = self
                .earliest_retry_at(
                    &request.key,
                    &request.model,
                    &request.protocols,
                    &request.tried,
                    request.response_affinity.as_deref(),
                    now_ms,
                    request.operation,
                )
                .filter(|at| *at > now_ms)
                .map(|at| Instant::now() + Duration::from_millis(at - now_ms));
            tokio::select! {
                _ = await_event(notified, earliest(deadline, due)) => {},
                _ = turn_changed => {},
            }
        }
    }

    /// Recovery waits share the same finite queue and accumulated budget as
    /// capacity waits. With no known due time, persistent mode waits for an
    /// actual pool event, not a fabricated one-second retry timer.
    #[expect(
        clippy::too_many_arguments,
        reason = "Recovery carries the same exact route and request budget as admission."
    )]
    pub(crate) async fn wait_for_recovery_event(
        &self,
        key: &AuthenticatedKey,
        model: &str,
        protocols: &[WireApi],
        exclusions: &HashSet<String>,
        response_affinity: Option<&str>,
        operation: RotationOperation,
        budget: &SharedRequestBudget,
        retry_deadline: Option<Instant>,
        persistent: bool,
    ) -> bool {
        let notified = self.candidate_availability.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if !budget.can_dispatch() || self.lock_scheduler().is_retired() {
            return false;
        }
        let Ok(queue_deadline) = self.admission_deadline(budget) else {
            return false;
        };
        let deadline = earliest(queue_deadline, retry_deadline);
        let now_ms = crate::unix_time_ms();
        let retry_at = self.recovery_retry_at(
            key,
            model,
            protocols,
            exclusions,
            response_affinity,
            now_ms,
            operation,
        );
        let due = match retry_at {
            Some(at) if at <= now_ms => return true,
            Some(at) => Some(Instant::now() + Duration::from_millis(at - now_ms)),
            None if persistent => None,
            None => return false,
        };
        if !persistent
            && due
                .zip(deadline)
                .is_some_and(|(due, deadline)| due > deadline)
        {
            return false;
        }
        let request = AdmissionRequest {
            key: key.clone(),
            model: model.into(),
            protocols: protocols.into(),
            tried: exclusions.clone(),
            response_affinity: response_affinity.map(str::to_owned),
            prompt_affinity: None,
            operation,
            budget: budget.clone(),
        };
        let _waiter = {
            let mut queue = self
                .admission
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let Some(waiter) = self.register_waiter(&mut queue, &request, false) else {
                return false;
            };
            waiter
        };
        await_event(notified, earliest(deadline, due)).await;
        !self.lock_scheduler().is_retired()
            && self.admission_deadline(budget).is_ok()
            && retry_deadline.is_none_or(|deadline| Instant::now() < deadline)
            && budget.can_dispatch()
    }
}

fn earliest(a: Option<Instant>, b: Option<Instant>) -> Option<Instant> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

async fn await_event(notified: impl std::future::Future<Output = ()>, deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::select! { _ = notified => {}, _ = sleep_until(at) => {} },
        None => notified.await,
    }
}
